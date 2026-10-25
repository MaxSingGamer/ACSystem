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
        fullname: String::new(),
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

/// 当前余额（读库口径）。
fn bal(conn: &rusqlite::Connection, uid: &str, atype: AccountType) -> i64 {
    account::require_account(conn, uid, atype).map(|a| a.balance).unwrap_or(-1)
}

/// 余额只能来自账本：测试也一律用「已确认的铸造」建底，不手写 balance。
fn mint_to(
    conn: &mut rusqlite::Connection,
    uid: &str,
    atype: AccountType,
    amt: i64,
) -> Result<Transaction> {
    let mut m = tx(TransactionType::Mint, "root", AccountType::System, uid, atype, amt);
    m.central_sig = Some("root-sig".into());
    transaction::submit_tx(conn, &m)?;
    Ok(m)
}

#[test]
fn transfer_needs_confirmation() -> Result<()> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    db::init_central(&conn)?;

    account::create_account(&conn, &acc("Alice", AccountType::Individual))?;
    account::create_account(&conn, &acc("Bob", AccountType::Individual))?;
    let fund = mint_to(&mut conn, "Alice", AccountType::Individual, 1000)?;

    let t = tx_chained(TransactionType::Transfer, "Alice", AccountType::Individual, "Bob", AccountType::Individual, 300,
                       Some(&fund.tx_hash), None);
    transaction::submit_tx(&mut conn, &t)?;
    // Pending 只冻结发送方（v3.2.1）：提交即扣发送方；接收方未点接受前不入账
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 700);
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 0);
    // 接收方确认（带确认签名）：此刻才入账 + 状态推进
    transaction::confirm_tx(&mut conn, &t.tx_id, "Bob", AccountType::Individual, "bob-sig")?;
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 700);
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 300);
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
    account::create_account(&conn, &acc("Alice", AccountType::Individual))?;
    account::create_account(&conn, &acc("Bob", AccountType::Individual))?;
    let fund = mint_to(&mut conn, "Alice", AccountType::Individual, 1000)?;

    let t = tx_chained(TransactionType::Transfer, "Alice", AccountType::Individual, "Bob", AccountType::Individual, 300,
                       Some(&fund.tx_hash), None);
    transaction::submit_tx(&mut conn, &t)?;
    // Pending 只冻结发送方（v3.2.1）：接收方拒收前不该有余额
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 700);
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 0);
    transaction::reject_tx(&mut conn, &t.tx_id, "Bob", AccountType::Individual, "不想收")?;
    // 拒收 → 发送方金额自动回退（Rejected 不计入余额）
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 1000);
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 0);
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
    account::create_account(&conn, &acc("Alice", AccountType::Individual))?;
    account::create_account(&conn, &acc("Bob", AccountType::Individual))?;
    let fund = mint_to(&mut conn, "Alice", AccountType::Individual, 1000)?;

    let t = tx_chained(TransactionType::Transfer, "Alice", AccountType::Individual, "Bob", AccountType::Individual, 300,
                       Some(&fund.tx_hash), None);
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

/// 确认时的可支付性复核：发送方**从头到尾就没有这笔钱**（如账本被篡改/铸造被删）时，
/// 接收方不得把这笔钱收进账。判定口径 = `raw_balance + 本笔金额 >= 0`
/// （本笔在提交时已冻结，故要把它加回来看「这笔钱在账本上是否真的存在」）。
#[test]
fn confirm_rejected_when_sender_never_had_funds() -> Result<()> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    db::init_central(&conn)?;
    account::create_account(&conn, &acc("Alice", AccountType::Individual))?;
    account::create_account(&conn, &acc("Bob", AccountType::Individual))?;

    // 账本来底 300；转出 200（提交即冻结 → Alice 只剩 100）
    let fund = mint_to(&mut conn, "Alice", AccountType::Individual, 300)?;
    let t = tx_chained(TransactionType::Transfer, "Alice", AccountType::Individual, "Bob", AccountType::Individual, 200,
                       Some(&fund.tx_hash), None);
    transaction::submit_tx(&mut conn, &t)?;
    // 模拟「发送方资产被抽空」：删掉铸造那笔 → Alice = 0-200 = -200，
    // 连本笔 200 都补不上（-200+200 = 0 < 0 不成立… 故再抽一笔已确认支出制造缺口）
    conn.execute("DELETE FROM transactions WHERE tx_id=?1", [fund.tx_id.as_str()])?;
    // 让缺口超过本笔：再挂一笔已确认的 100 支出（Alice 实际从未有过这笔钱）
    let ghost = tx_chained(TransactionType::Transfer, "Alice", AccountType::Individual,
                           "Bob", AccountType::Individual, 100, None, None);
    conn.execute(
        "INSERT INTO transactions(tx_id,tx_type,sender,sender_type,receiver,receiver_type,amount,ts,tx_hash,sender_sig,status) \
         VALUES(?1,'Transfer','Alice','Individual','Bob','Individual',100,?2,?3,'sig','Confirmed')",
        rusqlite::params![ghost.tx_id, ghost.timestamp, ghost.tx_hash],
    )?;
    // Alice raw_balance = -300 → -300 + 200 < 0：确认必须失败，Bob 不得入账
    assert!(transaction::confirm_tx(&mut conn, &t.tx_id, "Bob", AccountType::Individual, "sig").is_err());
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 0);
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
    account::create_account(&conn, &acc("Alice", AccountType::Individual))?;
    account::create_account(&conn, &acc("Bob", AccountType::Individual))?;
    let fund = mint_to(&mut conn, "Alice", AccountType::Individual, 1000)?;
    let t = tx_chained(TransactionType::Transfer, "Alice", AccountType::Individual, "Bob", AccountType::Individual, 100,
                       Some(&fund.tx_hash), None);
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
    //    receiver_head 必须取发行账户当前的链头（＝它上一笔 Issue 的哈希，
    //    因为发送方链头在**提交**时就已推进）
    let r = tx_chained(TransactionType::Redeem, "Alice", AccountType::Individual,
                       "PreIssuedAccount", AccountType::System, 100, Some(&i.tx_hash), Some(&i.tx_hash));
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

/// 接收方「未点接受就不算余额，点了接受才到账」（v3.2.1 口径）。
///
/// 发送方仍按「提交即冻结」扣减（Pending 支出计入，防同一笔钱重复花出），
/// 但 **Pending 收款不计入接收方** —— 待收箱里的一笔钱在点接受之前不属于接收方。
#[test]
fn pending_incoming_not_credited_until_accepted() -> Result<()> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    db::init_central(&conn)?;
    account::create_account(&conn, &acc("PreIssuedAccount", AccountType::System))?;
    account::create_account(&conn, &acc("Alice", AccountType::Individual))?;

    // 发行账户先有 1000（发行源）
    let mut m = tx(TransactionType::Mint, "root", AccountType::System,
                   "PreIssuedAccount", AccountType::System, 1000);
    m.central_sig = Some("root-sig".into());
    transaction::submit_tx(&mut conn, &m)?;

    // 预发行账户分发 400 给 Alice（Issue → Pending，等接收方确认）
    let i = tx_chained(TransactionType::Issue, "PreIssuedAccount", AccountType::System,
                       "Alice", AccountType::Individual, 400, Some(&m.tx_hash), None);
    transaction::submit_tx(&mut conn, &i)?;

    // 分发方（发送方）：提交即冻结 —— Issue 不改发行账户余额（机制如此），链头已推进
    assert_eq!(bal(&conn, "PreIssuedAccount", AccountType::System), 1000);
    // 接收方：**还没来得及点接受，不算余额**
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 0);
    // 待收箱里躺着这一笔
    assert_eq!(transaction::list_pending_for(&conn, "Alice", AccountType::Individual)?.len(), 1);

    // 点「接受」→ 状态 Confirmed、余额到账、待收箱清空
    transaction::confirm_tx(&mut conn, &i.tx_id, "Alice", AccountType::Individual, "alice-sig")?;
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 400);
    assert!(transaction::list_pending_for(&conn, "Alice", AccountType::Individual)?.is_empty());
    assert_eq!(
        transaction::get_transaction(&conn, &i.tx_id)?.unwrap().status,
        TransactionStatus::Confirmed
    );

    // 未接受前拒收：接收方余额始终为 0，待收箱也清空
    let i2 = tx_chained(TransactionType::Issue, "PreIssuedAccount", AccountType::System,
                        "Alice", AccountType::Individual, 100, Some(&i.tx_hash), None);
    transaction::submit_tx(&mut conn, &i2)?;
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 400);
    transaction::reject_tx(&mut conn, &i2.tx_id, "Alice", AccountType::Individual, "不要")?;
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 400);

    // 全量重算必须与增量口径一致
    account::recompute_all_balances(&conn)?;
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 400);
    assert_eq!(account::recompute_account(&conn, "Alice", AccountType::Individual)?, 400);
    Ok(())
}

/// 接收方链头在「交易提交之后、点接受之前」被推进，接受仍必须成功（v3.2.1 修复）。
///
/// 回归历史 bug：待收交易落库时快照了接收方当时的链头，而接收方只要自己转账、
/// 或先接受了另一笔收款，链头就会前移；确认时若硬性校验「链头一致」，
/// 这笔交易就永久卡在 Pending、点「接受」也不清理待收箱。
#[test]
fn receiver_can_confirm_after_head_moved() -> Result<()> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    db::init_central(&conn)?;
    account::create_account(&conn, &acc("Alice", AccountType::Individual))?;
    account::create_account(&conn, &acc("Bob", AccountType::Individual))?;
    let fund = mint_to(&mut conn, "Alice", AccountType::Individual, 3000)?;

    // Bob 先有一笔已接受的收入 200（链头推进到 f1）
    let f1 = tx_chained(TransactionType::Transfer, "Alice", AccountType::Individual,
                        "Bob", AccountType::Individual, 200, Some(&fund.tx_hash), None);
    transaction::submit_tx(&mut conn, &f1)?;
    transaction::confirm_tx(&mut conn, &f1.tx_id, "Bob", AccountType::Individual, "sig1")?;
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 200);

    // f2 与 f3 都在 Bob 链头 = f1 时落库（都 Pending，均不计入 Bob 余额）
    let f2 = tx_chained(TransactionType::Transfer, "Alice", AccountType::Individual,
                        "Bob", AccountType::Individual, 500, Some(&f1.tx_hash), Some(&f1.tx_hash));
    transaction::submit_tx(&mut conn, &f2)?;
    let f3 = tx_chained(TransactionType::Transfer, "Alice", AccountType::Individual,
                        "Bob", AccountType::Individual, 300, Some(&f2.tx_hash), Some(&f1.tx_hash));
    transaction::submit_tx(&mut conn, &f3)?;
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 200);
    assert_eq!(transaction::list_pending_for(&conn, "Bob", AccountType::Individual)?.len(), 2);

    // Bob 先接受 f3：链头被推进 → f2 的链头快照已过时
    transaction::confirm_tx(&mut conn, &f3.tx_id, "Bob", AccountType::Individual, "sig3")?;
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 500);
    // 再接受 f2：**必须成功**（此前会报「接收方链头不一致」永久卡住）
    transaction::confirm_tx(&mut conn, &f2.tx_id, "Bob", AccountType::Individual, "sig2")?;
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 1000);
    // 待收箱必须清空
    assert!(transaction::list_pending_for(&conn, "Bob", AccountType::Individual)?.is_empty());

    // 全量重算与增量口径一致
    account::recompute_all_balances(&conn)?;
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 1000);
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 2000);
    Ok(())
}

/// 多笔待收：乱序接受也必须都能落地（每笔都要能推进状态、清空待收箱）。
#[test]
fn multiple_pending_can_be_accepted_in_any_order() -> Result<()> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    db::init_central(&conn)?;
    account::create_account(&conn, &acc("Alice", AccountType::Individual))?;
    account::create_account(&conn, &acc("Bob", AccountType::Individual))?;
    let fund = mint_to(&mut conn, "Alice", AccountType::Individual, 1000)?;

    let t1 = tx_chained(TransactionType::Transfer, "Alice", AccountType::Individual,
                        "Bob", AccountType::Individual, 100, Some(&fund.tx_hash), None);
    transaction::submit_tx(&mut conn, &t1)?;
    let t2 = tx_chained(TransactionType::Transfer, "Alice", AccountType::Individual,
                        "Bob", AccountType::Individual, 200, Some(&t1.tx_hash), None);
    transaction::submit_tx(&mut conn, &t2)?;
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 0);
    assert_eq!(transaction::list_pending_for(&conn, "Bob", AccountType::Individual)?.len(), 2);

    // 后一笔先接受，前一笔后接受
    transaction::confirm_tx(&mut conn, &t2.tx_id, "Bob", AccountType::Individual, "sig2")?;
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 200);
    transaction::confirm_tx(&mut conn, &t1.tx_id, "Bob", AccountType::Individual, "sig1")?;
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 300);
    assert!(transaction::list_pending_for(&conn, "Bob", AccountType::Individual)?.is_empty());
    account::recompute_all_balances(&conn)?;
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 300);
    Ok(())
}

/// 余额纠偏体检的「按需触发」：无新增交易时不得重复全量重算（`/api/sync` 是免认证热路径）。
#[test]
fn balance_sanity_runs_only_when_ledger_changed() -> Result<()> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    db::init_central(&conn)?;
    account::create_account(&conn, &acc("Alice", AccountType::Individual))?;
    account::create_account(&conn, &acc("Bob", AccountType::Individual))?;

    // 首启无水位 → 必须体检一次（并建立水位）
    assert!(account::balance_dirty(&conn)?, "首启应视为需要体检");
    assert!(account::recompute_all_if_dirty(&conn)?, "首启应执行一次全量重算");
    // 紧接着再问：账本没变 → 不应再重算
    assert!(!account::balance_dirty(&conn)?);
    assert!(!account::recompute_all_if_dirty(&conn)?, "无新增交易不应重复全量重算");

    // 账本变了（新增一笔确认铸造）→ 重新需要体检，且重算把余额写对
    let fund = mint_to(&mut conn, "Alice", AccountType::Individual, 1000)?;
    assert!(account::balance_dirty(&conn)?, "新增交易后应需要体检");
    assert!(account::recompute_all_if_dirty(&conn)?);
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 1000);
    assert!(!account::balance_dirty(&conn)?);

    // 待收（Pending 收款）不入接收方余额，但同样会让水位前进
    let t = tx_chained(TransactionType::Transfer, "Alice", AccountType::Individual,
                       "Bob", AccountType::Individual, 250, Some(&fund.tx_hash), None);
    transaction::submit_tx(&mut conn, &t)?;
    assert!(account::balance_dirty(&conn)?);
    assert!(account::recompute_all_if_dirty(&conn)?);
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 750);
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 0);
    assert!(!account::balance_dirty(&conn)?);
    Ok(())
}

/// 余额口径（v3.2.1 起）：**发送方**的 `Pending` 支出立即冻结计入，
/// **接收方**只在 `Confirmed` 后计入；`Rejected` / `Error` 双方都不计入。
/// 并锁定「余额只能来自账本」这一不变量（任何绕过账本直接写 balance 的做法，
/// 都会在下一次全量重算时被抹掉）。
#[test]
fn balance_freezes_outgoing_but_not_pending_incoming() -> Result<()> {
    let mut conn = rusqlite::Connection::open_in_memory()?;
    db::init_central(&conn)?;
    account::create_account(&conn, &acc("Alice", AccountType::Individual))?;
    account::create_account(&conn, &acc("Bob", AccountType::Individual))?;

    // 用一笔已确认的铸造给 Alice 建底（重算只认账本，不认手工写入的 balance）
    let fund = mint_to(&mut conn, "Alice", AccountType::Individual, 1000)?;
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 1000);

    // 手工改余额 → 重算必须把它抹回账本口径（这是设计不变量，不是 bug）
    conn.execute("UPDATE accounts_individual SET balance=999999 WHERE uid='Alice'", [])?;
    account::recompute_all_balances(&conn)?;
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 1000);

    // 1) Pending 转账 100：提交即冻结发送方（接收方未接受，不入账）
    let pending = tx_chained(TransactionType::Transfer, "Alice", AccountType::Individual,
                             "Bob", AccountType::Individual, 100, Some(&fund.tx_hash), None);
    transaction::submit_tx(&mut conn, &pending)?;
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 900);
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 0);

    // 2) 拒收 → 发送方金额自动回退（Rejected 不计入余额）
    transaction::reject_tx(&mut conn, &pending.tx_id, "Bob", AccountType::Individual, "不要")?;
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 1000);
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 0);

    // 3) 再提一笔 300 → 置为 Error（同样不计入）；链头因拒收已回退到 fund.tx_hash
    let err = tx_chained(TransactionType::Transfer, "Alice", AccountType::Individual,
                         "Bob", AccountType::Individual, 300, Some(&fund.tx_hash), None);
    transaction::submit_tx(&mut conn, &err)?;
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 700);
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 0);
    transaction::mark_error(&mut conn, &err.tx_id)?;
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 1000);
    assert_eq!(bal(&conn, "Bob", AccountType::Individual), 0);

    // 4) 全量重算 / 单账户重算必须与增量口径一致
    account::recompute_all_balances(&conn)?;
    assert_eq!(bal(&conn, "Alice", AccountType::Individual), 1000);
    assert_eq!(account::recompute_account(&conn, "Alice", AccountType::Individual)?, 1000);
    assert_eq!(account::recompute_account(&conn, "Bob", AccountType::Individual)?, 0);
    Ok(())
}
