use std::mem::size_of;
use std::num::NonZeroU64;
use std::str::FromStr;

use bumpalo::Bump;
use solana_program::sysvar::SysvarId;
use solana_sdk::{bpf_loader, system_instruction};
use solana_sdk::account_info::AccountInfo;
use solana_sdk::clock::Epoch;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::program_pack::Pack;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::rent::Rent;
use solana_sdk::signer::keypair::Keypair;
use solana_sdk::signer::Signer;
use solana_sdk::system_program;
use solana_sdk::sysvar;
use solana_sdk::sysvar::Sysvar;
use solana_sdk::transaction::Transaction;
use solana_validator::test_validator::TestValidatorGenesis;
use spl_token::state::{Account, AccountState, Mint};

use serum_dex::instruction::{init_open_orders, initialize_market, MarketInstruction, new_order, NewOrderInstructionV3, SelfTradeBehavior};
use serum_dex::matching::{OrderType, Side};
use serum_dex::state::{gen_vault_signer_key, MarketState, OpenOrders};

/// Runs serum-dex instructions against a local test validator
///
/// Useful for easily profiling compute units of each instruction
#[test]
fn test_serum_dex() -> anyhow::Result<()> {
    solana_logger::setup_with_default("solana_runtime::message_processor=debug");

    const MARKET_LEN: u64 = 388;
    const BIDS_LEN: u64 = 8388620;
    const ASKS_LEN: u64 = 8388620;
    const REQ_Q_LEN: u64 = 652;
    const EVENT_Q_LEN: u64 = 65548;
    const OPEN_ORDERS_LEN: u64 = 3228;

    let market = Keypair::new();
    let program_id = Pubkey::from_str("9xQeWvG816bUx9EPjHmaT23yvVM2ZWbrrpZb9PusVFin")?;
    let coin_mint = Keypair::new();
    let pc_mint = Keypair::new();
    let coin_vault = Keypair::new();
    let pc_vault = Keypair::new();

    let bids = Keypair::new();
    let asks = Keypair::new();
    let req_q = Keypair::new();
    let event_q = Keypair::new();

    let coin_lot_size = 1;
    let pc_lot_size = 1;

    let open_orders_maker = Keypair::new();
    let open_orders_taker = Keypair::new();

    let bump = Bump::new();
    let mut i = 0;
    let (vault_signer_nonce, vault_signer_pk) = loop {
        assert!(i < 100);
        if let Ok(pk) = gen_vault_signer_key(i, &market.pubkey(), &program_id) {
            break (i, bump.alloc(pk));
        }
        i += 1;
    };

    let pc_dust_threshold = 5;

    let (test_validator, payer) = TestValidatorGenesis::default()
        .add_program("serum_dex", program_id)
        .start();
    let rpc_client = test_validator.get_rpc_client();
    let blockhash = rpc_client.get_latest_blockhash()?;

    // Create Market account
    let market_tx = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &payer.pubkey(),
                &market.pubkey(),
                rpc_client.get_minimum_balance_for_rent_exemption(388)?,
                388 as u64,
                &program_id,
            )
        ],
        Some(&payer.pubkey()),
        &vec![&payer, &market],
        blockhash,
    );
    rpc_client.send_and_confirm_transaction(&market_tx)?;

    // Create Bids account
    let bids_tx = Transaction::new_signed_with_payer(
        &[system_instruction::create_account(
            &payer.pubkey(),
            &bids.pubkey(),
            rpc_client.get_minimum_balance_for_rent_exemption(BIDS_LEN as usize)?,
            BIDS_LEN,
            &program_id,
        )],
        Some(&payer.pubkey()),
        &vec![&payer, &bids],
        blockhash,
    );
    rpc_client.send_and_confirm_transaction(&bids_tx)?;

    // Create Asks account
    let asks_tx = Transaction::new_signed_with_payer(
        &[system_instruction::create_account(
            &payer.pubkey(),
            &asks.pubkey(),
            rpc_client.get_minimum_balance_for_rent_exemption(ASKS_LEN as usize)?,
            ASKS_LEN,
            &program_id,
        )],
        Some(&payer.pubkey()),
        &vec![&payer, &asks],
        blockhash,
    );
    rpc_client.send_and_confirm_transaction(&asks_tx)?;

    // Create Request Queue account
    let req_q_tx = Transaction::new_signed_with_payer(
        &[system_instruction::create_account(
            &payer.pubkey(),
            &req_q.pubkey(),
            rpc_client.get_minimum_balance_for_rent_exemption(REQ_Q_LEN as usize)?,
            REQ_Q_LEN,
            &program_id,
        )],
        Some(&payer.pubkey()),
        &vec![&payer, &req_q],
        blockhash,
    );
    rpc_client.send_and_confirm_transaction(&req_q_tx)?;

    // Create Event Queue account
    let event_q_tx = Transaction::new_signed_with_payer(
        &[system_instruction::create_account(
            &payer.pubkey(),
            &event_q.pubkey(),
            rpc_client.get_minimum_balance_for_rent_exemption(EVENT_Q_LEN as usize)?,
            EVENT_Q_LEN,
            &program_id,
        )],
        Some(&payer.pubkey()),
        &vec![&payer, &event_q],
        blockhash,
    );
    rpc_client.send_and_confirm_transaction(&event_q_tx)?;

    // mints
    let blockhash = rpc_client.get_latest_blockhash().unwrap();
    let mut mint_tx = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &payer.pubkey(),
                &coin_mint.pubkey(),
                rpc_client
                    .get_minimum_balance_for_rent_exemption(spl_token::state::Mint::get_packed_len())?,
                Mint::get_packed_len() as u64,
                &spl_token::id(),
            ),
            spl_token::instruction::initialize_mint(
                &spl_token::id(),
                &coin_mint.pubkey(),
                &payer.pubkey(),
                None,
                2,
            )?,
            system_instruction::create_account(
                &payer.pubkey(),
                &pc_mint.pubkey(),
                rpc_client
                    .get_minimum_balance_for_rent_exemption(spl_token::state::Mint::get_packed_len())?,
                spl_token::state::Mint::get_packed_len() as u64,
                &spl_token::id(),
            ),
            spl_token::instruction::initialize_mint(
                &spl_token::id(),
                &pc_mint.pubkey(),
                &payer.pubkey(),
                None,
                2,
            )?,
        ],
        Some(&payer.pubkey()),
        &vec![&payer, &coin_mint, &pc_mint],
        blockhash,
    );
    mint_tx.sign(&vec![&payer, &coin_mint, &pc_mint], blockhash);
    rpc_client.send_and_confirm_transaction(&mint_tx)?;

    // vaults
    let blockhash = rpc_client.get_latest_blockhash().unwrap();
    let mut token_tx = Transaction::new_signed_with_payer(
        &[
            system_instruction::create_account(
                &payer.pubkey(),
                &coin_vault.pubkey(),
                rpc_client
                    .get_minimum_balance_for_rent_exemption(spl_token::state::Account::get_packed_len())?,
                spl_token::state::Account::get_packed_len() as u64,
                &spl_token::id(),
            ),
            spl_token::instruction::initialize_account(
                &spl_token::id(),
                &coin_vault.pubkey(),
                &coin_mint.pubkey(),
                vault_signer_pk,
            )?,
            spl_token::instruction::mint_to(
                &spl_token::id(),
                &coin_mint.pubkey(),
                &coin_vault.pubkey(),
                &payer.pubkey(),
                &[],
                42,
            )?,
            system_instruction::create_account(
                &payer.pubkey(),
                &pc_vault.pubkey(),
                rpc_client
                    .get_minimum_balance_for_rent_exemption(spl_token::state::Account::get_packed_len())?,
                spl_token::state::Account::get_packed_len() as u64,
                &spl_token::id(),
            ),
            spl_token::instruction::initialize_account(
                &spl_token::id(),
                &pc_vault.pubkey(),
                &pc_mint.pubkey(),
                vault_signer_pk,
            )?,
            spl_token::instruction::mint_to(
                &spl_token::id(),
                &pc_mint.pubkey(),
                &pc_vault.pubkey(),
                &payer.pubkey(),
                &[],
                42,
            )?,
        ],
        Some(&payer.pubkey()),
        &vec![&payer, &coin_vault, &pc_vault],
        blockhash,
    );
    token_tx.sign(&vec![&payer, &coin_vault, &pc_vault], blockhash);
    rpc_client.send_and_confirm_transaction(&token_tx)?;

    let coin_mint_account = spl_token::state::Mint::unpack(&rpc_client.get_account(&coin_mint.pubkey())?.data)?;
    println!("{:?}", coin_mint_account);

    let coin_vault_account = spl_token::state::Account::unpack(&rpc_client.get_account(&coin_vault.pubkey())?.data)?;
    println!("{:?}", coin_vault_account);

    let pc_mint_account = spl_token::state::Mint::unpack(&rpc_client.get_account(&pc_mint.pubkey())?.data)?;
    println!("{:?}", pc_mint_account);

    let pc_vault_account = spl_token::state::Account::unpack(&rpc_client.get_account(&pc_vault.pubkey())?.data)?;
    println!("{:?}", pc_vault_account);

    let blockhash = rpc_client.get_latest_blockhash().unwrap();
    let mut initialize_market_tx = Transaction::new_with_payer(
        &[
            initialize_market(
                &market.pubkey(),
                &program_id,
                &coin_mint.pubkey(),
                &pc_mint.pubkey(),
                &coin_vault.pubkey(),
                &pc_vault.pubkey(),
                None,
                None,
                None,
                &bids.pubkey(),
                &asks.pubkey(),
                &req_q.pubkey(),
                &event_q.pubkey(),
                coin_lot_size,
                pc_lot_size,
                vault_signer_nonce,
                pc_dust_threshold,
            )?
        ],
        Some(&payer.pubkey()),
    );
    initialize_market_tx.sign(&vec![&payer], blockhash);
    rpc_client.send_and_confirm_transaction(&initialize_market_tx)?;

    let mut market_data = rpc_client.get_account(&market.pubkey())?.data;
    println!("Market data {:?}", market_data);

    let blockhash = rpc_client.get_latest_blockhash().unwrap();
    let mut initialize_open_orders_tx = Transaction::new_with_payer(
        &[
            system_instruction::create_account(
                &payer.pubkey(),
                &open_orders_maker.pubkey(),
                rpc_client
                    .get_minimum_balance_for_rent_exemption(OPEN_ORDERS_LEN as usize)?,
                OPEN_ORDERS_LEN,
                &program_id,
            ),
            init_open_orders(
                &program_id,
                &open_orders_maker.pubkey(),
                &payer.pubkey(),
                &market.pubkey(),
                None,
            )?,
            system_instruction::create_account(
                &payer.pubkey(),
                &open_orders_taker.pubkey(),
                rpc_client
                    .get_minimum_balance_for_rent_exemption(OPEN_ORDERS_LEN as usize)?,
                OPEN_ORDERS_LEN,
                &program_id,
            ),
            init_open_orders(
                &program_id,
                &open_orders_taker.pubkey(),
                &payer.pubkey(),
                &market.pubkey(),
                None,
            )?

        ],
        Some(&payer.pubkey()),
    );
    initialize_open_orders_tx.sign(&vec![&payer], blockhash);
    rpc_client.send_and_confirm_transaction(&initialize_open_orders_tx)?;

    let mut market_data = rpc_client.get_account(&market.pubkey())?.data;
    println!("Market data {:?}", market_data);


    let blockhash = rpc_client.get_latest_blockhash().unwrap();
    let mut new_order_tx = Transaction::new_with_payer(
        &[
            new_order(
                &market.pubkey(),
                &open_orders_maker,
                &req_q.pubkey(),
                &event_q.pubkey(),
                &bids.pubkey(),
                &asks.pubkey(),
                &payer.pubkey(),
                &open_orders_maker.pubkey(),
                &coin_vault.pubkey(),
                &pc_vault.pubkey(),
                spl_token::id(),
                spl_token::id(),
                spl_token::id(),
                &Rent::id(),
                Side::Bid,
                NonZeroU64::new_unchecked(1000),
                NonZeroU64::new_unchecked(1100),
                OrderType::Limit,

            )?
        ],
        Some(&payer.pubkey()),
    );
    new_order_tx.sign(&vec![&payer], blockhash);
    rpc_client.send_and_confirm_transaction(&new_order_tx)?;

    let mut market_data = rpc_client.get_account(&market.pubkey())?.data;
    println!("Market data {:?}", market_data);

    Ok(())
}
