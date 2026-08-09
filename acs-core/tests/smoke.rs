//! acs-core 冒烟测试：分表路由 / 双方确认结算 / 余额校验 / 铸造自动确认 / 拒绝。

use acs_core::account;
use acs_core::db;
use acs_core::errors::Result;
use acs_core::models::*;
use acs_core::transaction;

fn acc(uid: &str, atype: AccountType) -> Account {
    Account {
        uid: uid.to_string(),
        account_type: atype,
        email: format!("{}@qq.com", uid),
        pubkey: "pubkey-placeholder".into(),
        encrypted_seckey: "secret-placeholder".into(),
        balance: 0,
        status: AccountStatus::Active,
        last_tx_hash: None,
        created_at: chrono::Utc::now(),
        changed_at: chrono::Utc::now(),
    }
}

fn tx(tx_type: TransactionType, s: &str, st: AccountType, r: &str, rt: AccountType, amt: i64) -> Transaction {
    let mut t = Transaction::new(tx_type, s.into(), st, r.into(), rt, amt);
    t.tx_hash = transaction::compute_tx_hash(&t);
    t.sender_sig = "sender-sig".into();
    t
}

#[test]
fn transfer_needs_confirmation() -> Result<()> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    db::init_central(&conn)?;

    let mut alice = acc("Alice", AccountType::Individual);
    alice.balance = 1000;
    let bob = acc("Bob", AccountType::Individual);
    account::create_account(&conn, &alice)?;
    account::create_account(&conn, &bob)?;

    let t = tx(TransactionType::Transfer, "Alice", AccountType::Individual, "Bob", AccountType::Individual, 300);
    transaction::submit_tx(&mut conn, &t)?;
    // Pending：余额未动
    let alice2 = account::require_account(&conn, "Alice", AccountType::Individual)?;
    assert_eq!(alice2.balance, 1000);
    // 接收方确认
    transaction::confirm_tx(&mut conn, &t.tx_id, "Bob", AccountType::Individual)?;
    let alice3 = account::require_account(&conn, "Alice", AccountType::Individual)?;
    let bob3 = account::require_account(&conn, "Bob", AccountType::Individual)?;
    assert_eq!(alice3.balance, 700);
    assert_eq!(bob3.balance, 300);
    let tx3 = transaction::get_transaction(&conn, &t.tx_id)?.unwrap();
    assert_eq!(tx3.status, TransactionStatus::Confirmed);
    Ok(())
}

#[test]
fn transfer_rejected_by_receiver() -> Result<()> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    db::init_central(&conn)?;
    let mut alice = acc("Alice", AccountType::Individual);
    alice.balance = 1000;
    let bob = acc("Bob", AccountType::Individual);
    account::create_account(&conn, &alice)?;
    account::create_account(&conn, &bob)?;

    let t = tx(TransactionType::Transfer, "Alice", AccountType::Individual, "Bob", AccountType::Individual, 300);
    transaction::submit_tx(&mut conn, &t)?;
    transaction::reject_tx(&mut conn, &t.tx_id, "Bob", AccountType::Individual, "不想收")?;
    let alice2 = account::require_account(&conn, "Alice", AccountType::Individual)?;
    assert_eq!(alice2.balance, 1000);
    let tx2 = transaction::get_transaction(&conn, &t.tx_id)?.unwrap();
    assert_eq!(tx2.status, TransactionStatus::Rejected);
    Ok(())
}

#[test]
fn only_receiver_can_confirm() -> Result<()> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    db::init_central(&conn)?;
    let mut alice = acc("Alice", AccountType::Individual);
    alice.balance = 1000;
    let bob = acc("Bob", AccountType::Individual);
    account::create_account(&conn, &alice)?;
    account::create_account(&conn, &bob)?;

    let t = tx(TransactionType::Transfer, "Alice", AccountType::Individual, "Bob", AccountType::Individual, 300);
    transaction::submit_tx(&mut conn, &t)?;
    // 发送方不能确认
    assert!(transaction::confirm_tx(&mut conn, &t.tx_id, "Alice", AccountType::Individual).is_err());
    Ok(())
}

#[test]
fn mint_auto_confirms_to_system() -> Result<()> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    db::init_central(&conn)?;
    let pre = acc("PreIssuedAccount", AccountType::System);
    account::create_account(&conn, &pre)?;

    let mut t = tx(TransactionType::Mint, "max_shin-root", AccountType::System, "PreIssuedAccount", AccountType::System, 5000);
    t.central_sig = Some("root-sig".into());
    transaction::submit_tx(&mut conn, &t)?;
    let pre2 = account::require_account(&conn, "PreIssuedAccount", AccountType::System)?;
    assert_eq!(pre2.balance, 5000);
    let tx2 = transaction::get_transaction(&conn, &t.tx_id)?.unwrap();
    assert_eq!(tx2.status, TransactionStatus::Confirmed);
    Ok(())
}

#[test]
fn mint_without_sig_rejected() -> Result<()> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    db::init_central(&conn)?;
    let pre = acc("PreIssuedAccount", AccountType::System);
    account::create_account(&conn, &pre)?;
    let t = tx(TransactionType::Mint, "max_shin-root", AccountType::System, "PreIssuedAccount", AccountType::System, 1);
    assert!(transaction::submit_tx(&mut conn, &t).is_err());
    Ok(())
}

#[test]
fn insufficient_balance_rejected_at_confirm() -> Result<()> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    db::init_central(&conn)?;
    let mut alice = acc("Alice", AccountType::Individual);
    alice.balance = 300;
    let bob = acc("Bob", AccountType::Individual);
    account::create_account(&conn, &alice)?;
    account::create_account(&conn, &bob)?;

    // 提交时余额够（300>=200），确认前被扣空 → 确认时拒绝
    let t = tx(TransactionType::Transfer, "Alice", AccountType::Individual, "Bob", AccountType::Individual, 200);
    transaction::submit_tx(&mut conn, &t)?;
    // 直接改库扣减（模拟并发）
    conn.execute("UPDATE accounts_individual SET balance=0 WHERE uid='Alice'", [])?;
    assert!(transaction::confirm_tx(&mut conn, &t.tx_id, "Bob", AccountType::Individual).is_err());
    Ok(())
}

#[test]
fn account_types_route_to_own_tables() -> Result<()> {
    let conn = rusqlite::Connection::open_in_memory()?;
    db::init_central(&conn)?;
    account::create_account(&conn, &acc("GPC", AccountType::Country))?;
    account::create_account(&conn, &acc("AlphaBank", AccountType::Bank))?;
    account::create_account(&conn, &acc("Shin", AccountType::Individual))?;
    account::create_account(&conn, &acc("PreIssuedAccount", AccountType::System))?;

    for at in [AccountType::Country, AccountType::Bank, AccountType::Individual, AccountType::System] {
        let n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {}", at.table_name()), [], |r| r.get(0))?;
        assert_eq!(n, 1, "表 {} 应有 1 行", at.table_name());
    }
    assert!(account::get_account(&conn, "GPC", AccountType::Individual)?.is_none());
    Ok(())
}

#[test]
fn list_pending_for_receiver() -> Result<()> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    db::init_central(&conn)?;
    let mut alice = acc("Alice", AccountType::Individual);
    alice.balance = 1000;
    let bob = acc("Bob", AccountType::Individual);
    account::create_account(&conn, &alice)?;
    account::create_account(&conn, &bob)?;
    let t = tx(TransactionType::Transfer, "Alice", AccountType::Individual, "Bob", AccountType::Individual, 100);
    transaction::submit_tx(&mut conn, &t)?;
    let pending = transaction::list_pending_for(&conn, "Bob", AccountType::Individual)?;
    assert_eq!(pending.len(), 1);
    let none = transaction::list_pending_for(&conn, "Alice", AccountType::Individual)?;
    assert_eq!(none.len(), 0);
    Ok(())
}
