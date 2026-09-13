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
    // 接收方确认（带确认签名）
    transaction::confirm_tx(&mut conn, &t.tx_id, "Bob", AccountType::Individual, "bob-sig")?;
    let alice3 = account::require_account(&conn, "Alice", AccountType::Individual)?;
    let bob3 = account::require_account(&conn, "Bob", AccountType::Individual)?;
    assert_eq!(alice3.balance, 700);
    assert_eq!(bob3.balance, 300);
    let tx3 = transaction::get_transaction(&conn, &t.tx_id)?.unwrap();
    assert_eq!(tx3.status, TransactionStatus::Confirmed);
    // 确认凭据落库：接收方签名 + 确认时间（拒绝理由字段应为空）
    assert_eq!(tx3.receiver_sig.as_deref(), Some("bob-sig"));
    assert!(tx3.confirmed_at.is_some());
    assert!(tx3.reject_reason.is_none());
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
    // 拒收理由应写入主表（以前写在 tx_confirmations，且从无任何查询读取）
    assert_eq!(tx2.reject_reason.as_deref(), Some("不想收"));
    assert!(tx2.confirmed_at.is_some());
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
    assert!(transaction::confirm_tx(&mut conn, &t.tx_id, "Alice", AccountType::Individual, "sig").is_err());
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
    assert!(transaction::confirm_tx(&mut conn, &t.tx_id, "Bob", AccountType::Individual, "sig").is_err());
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
    let conn = rusqlite::Connection::open_in_memory()?;
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
    // 凭据表已删除、账户表不再保存 password_hash；仅保留同意标识/登录时间
    let cred_tbl: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='account_credentials'",
        [], |r| r.get(0))?;
    assert_eq!(cred_tbl, 0, "account_credentials 应已删除");
    let has_pw: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('accounts_company') WHERE name='password_hash'",
        [], |r| r.get(0))?;
    assert_eq!(has_pw, 0, "账户表不应再有 password_hash 列");
    let s_type: String = conn.query_row(
        "SELECT sender_type FROM transactions WHERE tx_id='t1'", [], |r| r.get(0))?;
    assert_eq!(s_type, "Company");
    Ok(())
}

/// 同名账户不得串账：铸造发出方 UID 与某个个人账户 UID 完全相同时，
/// 该铸造**不得**影响个人账户余额，也不得出现在它的流水里。
/// （历史 bug：铸造发出方曾取管理员登录 UID，与同名个人账户混记。）
#[test]
fn samename_mint_does_not_touch_individual_account() -> Result<()> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    db::init_central(&conn)?;
    // 个人账户与铸造发出方同名：uid 都是 Max_Shin，但类型不同
    account::create_account(&conn, &acc("Max_Shin", AccountType::Individual))?;
    account::create_account(&conn, &acc("PreIssuedAccount", AccountType::System))?;

    let mut m = tx(
        TransactionType::Mint,
        "Max_Shin",
        AccountType::System,
        "PreIssuedAccount",
        AccountType::System,
        500,
    );
    m.central_sig = Some("root-sig".into());
    transaction::submit_tx(&mut conn, &m)?;

    // 同名个人账户余额不受影响；发行账户收到铸造
    assert_eq!(
        account::require_account(&conn, "Max_Shin", AccountType::Individual)?.balance,
        0
    );
    assert_eq!(
        account::require_account(&conn, "PreIssuedAccount", AccountType::System)?.balance,
        500
    );
    // 该个人账户的流水（SQL 内即按 uid+type 过滤）不含这笔同名铸造
    let mine = transaction::list_transactions_for(&conn, "Max_Shin", AccountType::Individual)?;
    assert!(mine.is_empty(), "同名铸造不应出现在个人账户流水中：{mine:?}");

    // 全量重算也不能把它算进来
    account::recompute_all_balances(&conn)?;
    assert_eq!(
        account::require_account(&conn, "Max_Shin", AccountType::Individual)?.balance,
        0
    );
    assert_eq!(account::recompute_account(&conn, "Max_Shin", AccountType::Individual)?, 0);
    Ok(())
}

/// 带链头快照的交易构造（哈希含链头，故必须在设置链头后重算）。
fn tx_chained(
    tx_type: TransactionType,
    s: &str, st: AccountType, r: &str, rt: AccountType, amt: i64,
    sender_head: Option<&str>, receiver_head: Option<&str>,
) -> Transaction {
    let mut t = Transaction::new(tx_type, s.into(), st, r.into(), rt, amt);
    t.sender_last_hash = sender_head.map(str::to_string);
    t.receiver_last_hash = receiver_head.map(str::to_string);
    t.tx_hash = transaction::compute_tx_hash(&t);
    t.sender_sig = "sender-sig".into();
    t
}

/// 锁定 Issue / Redeem 的**单向口径**（这是货币机制，不是漏洞）：
/// - Issue（发行）＝ 商品篮子 → A€：收款方增加，**付款方（发行账户）余额不变**
/// - Redeem（赎回）＝ A€ → 商品篮子：付款方减少，**收款方（发行账户）余额不变**
/// 同时断言 `recompute_all_balances` / `recompute_account` 与结算时的增量结果完全一致，
/// 防止两处口径漂移（历史上这套不对称口径曾因缺少注释而被误判为 bug）。
#[test]
fn issue_and_redeem_are_one_sided() -> Result<()> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    db::init_central(&conn)?;
    account::create_account(&conn, &acc("PreIssuedAccount", AccountType::System))?;
    account::create_account(&conn, &acc("Alice", AccountType::Individual))?;
    account::create_account(&conn, &acc("Bob", AccountType::Individual))?;

    // 1) 铸造 1000 → 发行账户（自动确认）
    let mut m = tx(TransactionType::Mint, "root", AccountType::System,
                   "PreIssuedAccount", AccountType::System, 1000);
    m.central_sig = Some("root-sig".into());
    transaction::submit_tx(&mut conn, &m)?;
    assert_eq!(account::require_account(&conn, "PreIssuedAccount", AccountType::System)?.balance, 1000);

    // 2) 发行 300：发行账户 → Alice（发行账户余额不变）
    let i = tx_chained(TransactionType::Issue, "PreIssuedAccount", AccountType::System,
                       "Alice", AccountType::Individual, 300, Some(&m.tx_hash), None);
    transaction::submit_tx(&mut conn, &i)?;
    transaction::confirm_tx(&mut conn, &i.tx_id, "Alice", AccountType::Individual, "alice-sig")?;
    assert_eq!(account::require_account(&conn, "PreIssuedAccount", AccountType::System)?.balance, 1000);
    assert_eq!(account::require_account(&conn, "Alice", AccountType::Individual)?.balance, 300);

    // 3) 赎回 100：Alice → 发行账户（销毁，发行账户余额仍不变）
    let r = tx_chained(TransactionType::Redeem, "Alice", AccountType::Individual,
                       "PreIssuedAccount", AccountType::System, 100, Some(&i.tx_hash), Some(&m.tx_hash));
    transaction::submit_tx(&mut conn, &r)?;
    transaction::confirm_tx(&mut conn, &r.tx_id, "PreIssuedAccount", AccountType::System, "sys-sig")?;
    assert_eq!(account::require_account(&conn, "Alice", AccountType::Individual)?.balance, 200);
    assert_eq!(account::require_account(&conn, "PreIssuedAccount", AccountType::System)?.balance, 1000);

    // 4) 全量重算 / 单账户重算结果必须与增量结算一致
    account::recompute_all_balances(&conn)?;
    assert_eq!(account::require_account(&conn, "Alice", AccountType::Individual)?.balance, 200);
    assert_eq!(account::require_account(&conn, "PreIssuedAccount", AccountType::System)?.balance, 1000);
    assert_eq!(account::require_account(&conn, "Bob", AccountType::Individual)?.balance, 0);
    assert_eq!(account::recompute_account(&conn, "Alice", AccountType::Individual)?, 200);
    assert_eq!(account::recompute_account(&conn, "Bob", AccountType::Individual)?, 0);
    Ok(())
}

/// Pending / Rejected 交易一律不计入余额；并锁定「余额只能来自账本」这一不变量
/// （任何绕过账本直接写 balance 的做法，都会在下一次全量重算时被抹掉）。
#[test]
fn recompute_ignores_unconfirmed() -> Result<()> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    db::init_central(&conn)?;
    account::create_account(&conn, &acc("Alice", AccountType::Individual))?;
    account::create_account(&conn, &acc("Bob", AccountType::Individual))?;

    // 用一笔已确认的铸造给 Alice 建底（重算只认账本，不认手工写入的 balance）
    let mut fund = tx(TransactionType::Mint, "root", AccountType::System,
                      "Alice", AccountType::Individual, 1000);
    fund.central_sig = Some("root-sig".into());
    transaction::submit_tx(&mut conn, &fund)?;
    assert_eq!(account::require_account(&conn, "Alice", AccountType::Individual)?.balance, 1000);

    // 手工改余额 → 重算必须把它抹回账本口径（这是设计不变量，不是 bug）
    conn.execute("UPDATE accounts_individual SET balance=999999 WHERE uid='Alice'", [])?;
    account::recompute_all_balances(&conn)?;
    assert_eq!(account::require_account(&conn, "Alice", AccountType::Individual)?.balance, 1000);

    // Pending 与 Rejected 的转账都不影响余额
    let pending = tx_chained(TransactionType::Transfer, "Alice", AccountType::Individual,
                             "Bob", AccountType::Individual, 100,
                             Some(&fund.tx_hash), None);
    transaction::submit_tx(&mut conn, &pending)?;
    let rejected = tx_chained(TransactionType::Transfer, "Alice", AccountType::Individual,
                              "Bob", AccountType::Individual, 200,
                              Some(&fund.tx_hash), None);
    transaction::submit_tx(&mut conn, &rejected)?;
    transaction::reject_tx(&mut conn, &rejected.tx_id, "Bob", AccountType::Individual, "不要")?;

    account::recompute_all_balances(&conn)?;
    assert_eq!(account::require_account(&conn, "Alice", AccountType::Individual)?.balance, 1000);
    assert_eq!(account::require_account(&conn, "Bob", AccountType::Individual)?.balance, 0);
    Ok(())
}
