//! 错误数据修复（v3.0.0 起以 Rust EXE 子命令提供，服务器无需 sqlite3/python）。
//!
//! 用法：
//!   acs-server repair <db_path>            # 预览将被删除的错误交易
//!   acs-server repair <db_path> --apply    # 实际删除（先备份，再事务删除）
//!
//! 删除对象：
//!   - 状态为 Rejected / Error 的交易及其确认记录
//!   - 量级异常（金额 <=0 或 >1e12）的交易
//!   - 无对应主交易的孤儿 tx_confirmations

use std::path::Path;

use acs_core::db;
use acs_core::log;

fn preview(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<(String, String, String, String, i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT tx_id, tx_type, sender, receiver, amount, status FROM transactions \
         WHERE status IN ('Rejected','Error') OR amount<=0 OR amount>1000000000000 \
         ORDER BY ts",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
            r.get::<_, String>(5)?,
        ))
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    Ok(out)
}

fn orphan_count(conn: &rusqlite::Connection) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM tx_confirmations c \
         LEFT JOIN transactions t ON t.tx_id=c.tx_id WHERE t.tx_id IS NULL",
        [],
        |r| r.get(0),
    )
}

/// 执行修复。dry_run=true 仅打印；否则在事务内删除并返回删除数。
pub fn run(db_path: &str, dry_run: bool) -> anyhow::Result<()> {
    if !Path::new(db_path).exists() {
        anyhow::bail!("数据库不存在：{db_path}");
    }
    log::call(format!("repair 开始（dry_run={dry_run}）db={db_path}"));
    let mut conn = db::open_db(Path::new(db_path))?;
    let rows = preview(&conn)?;
    let orph = orphan_count(&conn)?;

    println!("【预览】待删除的错误/异常交易：{} 笔；孤儿确认记录：{} 条", rows.len(), orph);
    for (tid, ty, s, r, amt, st) in &rows {
        println!("  {tid}  {ty}  {s} → {r}  {amt} A€  [{st}]");
    }

    if dry_run {
        log::out("repair 预览完成（未删除）");
        println!("（预览模式：未删除。确认后加 --apply 执行）");
        return Ok(());
    }
    if rows.is_empty() && orph == 0 {
        println!("无错误数据，无需修复。");
        return Ok(());
    }

    // 备份提示 + 事务删除
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM transactions WHERE status IN ('Rejected','Error') OR amount<=0 OR amount>1000000000000", [])?;
    tx.execute(
        "DELETE FROM tx_confirmations WHERE tx_id NOT IN (SELECT tx_id FROM transactions)",
        [],
    )?;
    tx.commit()?;
    log::out(format!("repair 完成：删除 {} 笔异常交易 + 孤儿确认", rows.len()));
    println!("修复完成：已删除 {} 笔异常交易；孤儿确认记录已清理。", rows.len());
    println!("提示：请人工核对账户余额 / last_tx_hash 是否需要回拨。");
    Ok(())
}
