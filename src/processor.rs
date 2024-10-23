use solana_program::{
    pubkey::Pubkey,
    msg,
    account_info::{AccountInfo, next_account_info},
    entrypoint::ProgramResult,
    program_error::ProgramError,
    sysvar::{rent::Rent, Sysvar},
    program_pack::{Pack, IsInitialized},
    program::{invoke, invoke_signed},
};
use spl_token::state::Account as TokenAccount;
use crate::{
    error::EscrowError, 
    instruction::EscrowInstruction, 
    state::Escrow
};

pub struct Processor;

impl Processor {
    pub fn process(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        instruction_data: &[u8]
    ) -> ProgramResult {
        let instruction = EscrowInstruction::unpack(instruction_data)?;
        
        match instruction {
            EscrowInstruction::InitEscrow { amount } => {
                msg!("Instruction: InitEscrow");
                Self::process_init_escrow(accounts, amount, program_id)
            },
            EscrowInstruction::Exchange { amount } => {
                msg!("Instruction: Exchange");
                Self::process_exchange_escrow(accounts, amount, program_id)
            }
        }
    }

    fn process_init_escrow(
        accounts: &[AccountInfo],
        amount: u64,
        program_id: &Pubkey
    ) -> ProgramResult {

        let account_info_iter = &mut accounts.iter();

        let initializer = next_account_info(account_info_iter)?;
        if !initializer.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let temp_token_account = next_account_info(account_info_iter)?;

        let token_to_receive_account = next_account_info(account_info_iter)?;
        if *token_to_receive_account.owner != spl_token::id() {
            return Err(ProgramError::IncorrectProgramId);
        }

        let escrow_account = next_account_info(account_info_iter)?;
        
        let rent = Rent::get()?;

        if !rent.is_exempt(escrow_account.lamports(), escrow_account.data_len()) {
            return Err(EscrowError::NotRentExempt.into());
        }

        let mut escrow_info = Escrow::unpack_unchecked(&escrow_account.try_borrow_data()?)?;
        if escrow_info.is_initialized() {
            return Err(ProgramError::AccountAlreadyInitialized);
        }

        escrow_info.is_initialized = true;
        escrow_info.initializer_pubkey = *initializer.key;
        escrow_info.temp_token_account_pubkey = *temp_token_account.key;
        escrow_info.initializer_token_to_receive_account_pubkey = *token_to_receive_account.key;
        escrow_info.expected_amount = amount;

        Escrow::pack(escrow_info, &mut escrow_account.try_borrow_mut_data()?)?;

        let (pda, _bump_seed) = Pubkey::find_program_address(&[b"escrow"], program_id);

        let token_program = next_account_info(account_info_iter)?;
        let owner_change_ix 
            = spl_token::instruction::set_authority(
                token_program.key, 
                temp_token_account.key, 
                Some(&pda), 
                spl_token::instruction::AuthorityType::AccountOwner, 
                initializer.key, 
                &[initializer.key]
            )?;

        msg!("Calling the token program to transfer token account ownership...");
        invoke(
            &owner_change_ix, 
            &[
                temp_token_account.clone(),
                initializer.clone(),
                token_program.clone()
            ]
        )?;

        Ok(())
    }

    fn process_exchange_escrow(
        accounts: &[AccountInfo],
        amount: u64,
        program_id: &Pubkey
    ) -> ProgramResult {

        let account_info_iter = &mut accounts.iter();

        let taker = next_account_info(account_info_iter)?;
        let taker_sending_token_account = next_account_info(account_info_iter)?;
        let taker_token_to_receive_account = next_account_info(account_info_iter)?;
        let temp_token_account = next_account_info(account_info_iter)?;
        let initializer_main_account = next_account_info(account_info_iter)?;
        let initializer_token_to_receive_account = next_account_info(account_info_iter)?;
        let escrow_account = next_account_info(account_info_iter)?;
        let token_program = next_account_info(account_info_iter)?;
        let pda_account = next_account_info(account_info_iter)?;

        // We check the taker is signing the Transaction
        if !taker.is_signer {
            return Err(ProgramError::MissingRequiredSignature)
        }

        // We check the balance currently in temp_token_account
        let temp_token_account_info = TokenAccount::unpack(
            &temp_token_account.try_borrow_data()?
        )?;

        if temp_token_account_info.amount != amount {
            return Err(EscrowError::EscrowAmountMismatch.into())
        }

        let escrow_account_info = Escrow::unpack(&escrow_account.try_borrow_data()?)?;

        // we verify that initializer is correct, that temp token is correct, and that token to receive is correct
        if escrow_account_info.initializer_pubkey != *initializer_main_account.key {
            return Err(ProgramError::InvalidAccountData);
        }

        if escrow_account_info.initializer_token_to_receive_account_pubkey != *initializer_token_to_receive_account.key {
            return Err(ProgramError::InvalidAccountData);
        }

        if escrow_account_info.temp_token_account_pubkey != *temp_token_account.key {
            return Err(ProgramError::InvalidAccountData);
        }

        // transfer amount from taker to initializer
        let transfer_to_initializer_ix 
            = spl_token::instruction::transfer(
                token_program.key, 
                taker_sending_token_account.key, 
                initializer_token_to_receive_account.key, 
                taker.key, 
                &[taker.key], 
                escrow_account_info.expected_amount,
            )?;

        msg!("Calling the token program to transfer tokens to the escrow's initializer...");
        invoke(
            &transfer_to_initializer_ix, 
            &[
                taker_sending_token_account.clone(),
                initializer_token_to_receive_account.clone(),
                taker.clone(),
                token_program.clone()
            ]
        )?;

        let (pda, bump_seed) = Pubkey::find_program_address(
            &[b"escrow"], 
            program_id
        );

        // transfer to the taker
        let transfer_to_taker_ix 
            = spl_token::instruction::transfer(
                token_program.key, 
                temp_token_account.key,
                taker_token_to_receive_account.key,
                &pda,
                &[&pda],
                temp_token_account_info.amount,
            )?;

        msg!("Calling the token program to transfer tokens to the taker...");
        invoke_signed(
            &transfer_to_taker_ix, 
            &[
                temp_token_account.clone(),
                taker_token_to_receive_account.clone(),
                pda_account.clone(),
                token_program.clone(),
            ], 
            &[&[&b"escrow"[..], &[bump_seed]]]
        )?;

        // we close the temp account
        let close_temp_acc_ix = spl_token::instruction::close_account(
            token_program.key, 
            temp_token_account.key, 
            initializer_main_account.key, 
            &pda, 
            &[&pda]
        )?;

        msg!("Calling the token program to close pda's temp account...");
        invoke_signed(
            &close_temp_acc_ix, 
            &[
                temp_token_account.clone(),
                initializer_main_account.clone(),
                pda_account.clone(),
                token_program.clone(),
            ], 
            &[&[&b"escrow"[..], &[bump_seed]]]
        )?;

        // we close the escrow data account
        msg!("Closing the escrow account...");
        **initializer_main_account.lamports.borrow_mut() = initializer_main_account.lamports()
            .checked_add(escrow_account.lamports())
            .ok_or(EscrowError::AmountOverflow)?;
        **escrow_account.lamports.borrow_mut() = 0;
        *escrow_account.try_borrow_mut_data()? = &mut [];

        Ok(())
    }
}


