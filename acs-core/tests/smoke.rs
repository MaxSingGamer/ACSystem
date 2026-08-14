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
    account::create_account(&conn, &acc("AlphaCompany", AccountType::Company))?;
    account::create_account(&conn, &acc("Shin", AccountType::Individual))?;
    account::create_account(&conn, &acc("PreIssuedAccount", AccountType::System))?;

    for at in [AccountType::Country, AccountType::Company, AccountType::Individual, AccountType::System] {
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

#[test]
fn migrate_bank_to_company() -> Result<()> {
    // 构造旧库：accounts_bank 表 + account_credentials/transactions 中的 'Bank' 类型字符串
    let mut conn = rusqlite::Connection::open_in_memory()?;
    conn.execute_batch(
        "CREATE TABLE accounts_bank(
            uid TEXT PRIMARY KEY, email TEXT NOT NULL,
            pubkey TEXT NOT NULL, encrypted_seckey TEXT NOT NULL,
            balance INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL DEFAULT 'Active',
            last_tx_hash TEXT, created_at INTEGER NOT NULL, changed_at INTEGER NOT NULL);
         INSERT INTO accounts_bank(uid,email,pubkey,encrypted_seckey,balance,status,created_at,changed_at)
            VALUES('AlphaCompany','a@qq.com','pk','sk',42,'Active',1,1);
         CREATE TABLE account_credentials(uid TEXT, type TEXT, password_hash TEXT, PRIMARY KEY(uid,type));
         INSERT INTO account_credentials VALUES('AlphaCompany','Bank','hash');
         CREATE TABLE transactions(
            tx_id TEXT PRIMARY KEY, tx_type TEXT NOT NULL,
            sender TEXT NOT NULL, sender_type TEXT NOT NULL,
            receiver TEXT NOT NULL, receiver_type TEXT NOT NULL,
            amount INTEGER NOT NULL, ts INTEGER NOT NULL,
            tx_hash TEXT NOT NULL, sender_sig TEXT NOT NULL,
            central_sig TEXT, sender_last_hash TEXT, receiver_last_hash TEXT,
            status TEXT NOT NULL DEFAULT 'Pending');
         INSERT INTO transactions(tx_id,tx_type,sender,sender_type,receiver,receiver_type,amount,ts,tx_hash,sender_sig,status)
            VALUES('t1','Transfer','AlphaCompany','Bank','Alice','Individual',1,1,'h','s','Pending');",
    )?;
    // 模拟升级：新 schema 建表（创建 accounts_company）+ 迁移
    db::init_central(&conn)?;
    db::migrate_center(&conn)?;
    // accounts_bank 已删除，数据进入 accounts_company
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM accounts_company", [], |r| r.get(0))?;
    assert_eq!(n, 1, "accounts_company 应有 1 行");
    let bal: i64 = conn.query_row("SELECT balance FROM accounts_company WHERE uid='AlphaCompany'", [], |r| r.get(0))?;
    assert_eq!(bal, 42);
    let table_bank: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='accounts_bank'",
        [], |r| r.get(0))?;
    assert_eq!(table_bank, 0, "accounts_bank 应已删除");
    // 类型字符串迁移
    let cred_type: String = conn.query_row(
        "SELECT type FROM account_credentials WHERE uid='AlphaCompany'", [], |r| r.get(0))?;
    assert_eq!(cred_type, "Company");
    let s_type: String = conn.query_row(
        "SELECT sender_type FROM transactions WHERE tx_id='t1'", [], |r| r.get(0))?;
    assert_eq!(s_type, "Company");
    Ok(())
}
