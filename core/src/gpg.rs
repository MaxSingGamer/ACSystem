//! GnuPG（gpg.exe）封装。
//!
//! - 密钥为 ed25519（OpenPGP 格式），与 GnuPG 完全兼容，技术人员可用
//!   `gpg --homedir <~/.alphawallet/<中心域名>/gnupg> --list-keys` 直接审查。
//! - 私钥以"密码上锁"的 armored 格式导出（S2K+AES），可被 GnuPG 读取。
//! - 交互通过 `--batch --pinentry-mode loopback --passphrase-fd 0` 传密码，避免卡交互。
//! - 签名/验签的数据走临时文件，规避"密码与数据争用 stdin"的问题。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::errors::{AcsError, Result};
use crate::models::GeneratedKey;

/// 判断是否为「gpg-agent 仍指向已被删除/替换的 homedir」这类可自愈错误。
fn is_stale_agent_error(e: &AcsError) -> bool {
    let s = e.to_string();
    s.contains("agent_genkey failed") || s.contains("No such file or directory")
}

/// 判断是否为「信任库（trustdb.gpg）损坏」这类可自愈错误。
///
/// 现场成因：gpg/gpg-agent 的 stderr 被写进了 trustdb.gpg，导致文件头部变成
/// `\x01gpg\x03…` 明文日志，此后任何 `--list-keys` / `--quick-generate-key` 都报
/// `invalid record type` / `Trust DB error`。表现为「注册时生成钱包密钥失败」，
/// 且因为报错文案被上层 context 吞掉，用户只看到一句笼统提示。
///
/// 修复方式（不触碰任何密钥文件）：把损坏的信任库转存备份后删除，gpg 会自动重建；
/// 本机自有密钥的信任级别本来就是 ultimate，重建不丢信息。
fn is_trustdb_error(e: &AcsError) -> bool {
    let s = e.to_string().to_ascii_lowercase();
    s.contains("trust db error")
        || s.contains("trustdb is corrupted")
        || s.contains("invalid record type")
        || s.contains("trustdb")
}

/// gpg 对同 user_id 已有密钥时的提示短语（`--quick-generate-key` 会照旧新建，
/// 但不同版本行为有差异，故一并纳入「需要清理后重试」的判断）。
const ALREADY_EXISTS_HINT: &str = "already exists";

/// 陈旧锁文件视为可清理的年龄下限（秒）：避免删掉正在使用中的锁。
const STALE_LOCK_SECS: u64 = 60;

fn is_lock_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.ends_with(".lock"))
        .unwrap_or(false)
}

/// Windows 的 `CREATE_NO_WINDOW`：让子进程**不创建控制台窗口**。
///
/// 为什么必须显式设置：客户端是 GUI 子系统（双击启动无控制台），但每次 `gpg.exe`
/// （sign / genkeypair / export / gpgconf / powershell）都会**新开一个控制台窗口闪一下**。
/// `main.rs::hide_console()` 只能隐藏**自身**的控制台，管不到子进程。
///
/// 注：Linux / macOS 无控制台窗口概念，此处为空实现。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 给即将 spawn 的进程加上「无窗口」标志（仅 Windows 生效）。
fn hide_child_window(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

#[derive(Clone)]
pub struct GpgUtil {
    gpg_path: PathBuf,
    homedir: PathBuf,
}

impl GpgUtil {
    pub fn new(gpg_path: impl Into<PathBuf>, homedir: impl Into<PathBuf>) -> Self {
        GpgUtil {
            gpg_path: gpg_path.into(),
            homedir: homedir.into(),
        }
    }

    /// gpg homedir 的展示路径（诊断信息用；统一用 `/` 分隔，避免 Windows 反斜杠转义困扰）。
    pub fn homedir_display(&self) -> String {
        self.homedir.display().to_string().replace('\\', "/")
    }

    /// 原始 `--with-colons --list-keys` 输出（含 pub/fpr/uid 行），供重复密钥清理等场景解析。
    /// 信任库损坏时先自愈再重试。
    pub fn list_keys_colons(&self) -> Result<String> {
        let args = ["--with-colons", "--list-keys"];
        match self.run(&args, None, None) {
            Ok((out, _)) => Ok(out),
            Err(e) if is_trustdb_error(&e) => {
                self.repair_trustdb();
                let (out, _) = self.run(&args, None, None)?;
                Ok(out)
            }
            Err(e) => Err(e),
        }
    }

    /// 列出本机密钥：`(User ID, 指纹, 创建时间 Unix 秒)`，供重复密钥清理使用。
    /// 同一把密钥有多个 uid 时会产生多条记录（清理逻辑按 uid 分组，正好符合预期）。
    pub fn list_keys_with_uid(&self) -> Result<Vec<(String, String, i64)>> {
        let out = self.list_keys_colons()?;
        let mut keys: Vec<(String, String, i64)> = Vec::new();
        let mut cur: Option<(String, i64)> = None; // (fpr, created)
        for line in out.lines() {
            let cols: Vec<&str> = line.split(':').collect();
            if line.starts_with("pub:") {
                // pub 记录第 5 列（下标 4）为创建时间（Unix 秒）
                let ts = cols.get(4).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
                cur = Some((String::new(), ts));
            } else if line.starts_with("fpr:") {
                if let Some((f, _)) = cur.as_mut().filter(|(f, _)| f.is_empty()) {
                    if let Some(fp) = cols.get(9).filter(|s| !s.is_empty()) {
                        *f = fp.to_string();
                    }
                }
            } else if line.starts_with("uid:") {
                if let Some((f, ts)) = cur.as_ref().filter(|(f, _)| !f.is_empty()) {
                    keys.push((cols.get(9).unwrap_or(&"").to_string(), f.clone(), *ts));
                }
            }
        }
        Ok(keys)
    }

    /// 列出本机所有密钥指纹（`--list-keys` 的 fpr 行；损坏时先自愈再重试）。
    /// 供 `repair` 自检与重复密钥清理使用。
    pub fn list_fingerprints(&self) -> Result<Vec<String>> {
        let args = ["--with-colons", "--list-keys"];
        let (out, _) = match self.run(&args, None, None) {
            Ok(v) => v,
            Err(e) if is_trustdb_error(&e) => {
                self.repair_trustdb();
                self.run(&args, None, None)?
            }
            Err(e) => return Err(e),
        };
        // 只取公钥主键的 fpr 行：以 fpr 记录为界，紧跟 pub 之后的那条才是主指纹
        let mut out_fprs = Vec::new();
        let mut expect_fpr = false;
        for line in out.lines() {
            if line.starts_with("pub:") {
                expect_fpr = true;
            } else if line.starts_with("fpr:") && expect_fpr {
                if let Some(f) = line.split(':').nth(9).filter(|f| !f.is_empty()) {
                    out_fprs.push(f.to_string());
                }
                expect_fpr = false;
            } else if line.starts_with("sub:") || line.starts_with("uid:") {
                expect_fpr = false;
            }
        }
        Ok(out_fprs)
    }

    // ---- 底层执行 ----

    fn run(
        &self,
        args: &[&str],
        passphrase: Option<&str>,
        stdin_data: Option<&[u8]>,
    ) -> Result<(String, String)> {
        self.run_homedir(&self.homedir, args, passphrase, stdin_data)
    }

    fn run_homedir(
        &self,
        homedir: &Path,
        args: &[&str],
        passphrase: Option<&str>,
        stdin_data: Option<&[u8]>,
    ) -> Result<(String, String)> {
        let mut cmd = Command::new(&self.gpg_path);
        // GUI 程序里执行 gpg 不要弹控制台黑窗（v3.3.0 #12 / D23）
        hide_child_window(&mut cmd);
        // Windows 下用 --homedir 与已运行的 gpg-agent 冲突（agent_genkey failed），
        // 改为设置 GNUPGHOME 环境变量，gpg 会为该目录自动拉起独立 agent。
        cmd.env("GNUPGHOME", homedir)
            .arg("--batch")
            .arg("--yes")
            .arg("--pinentry-mode")
            .arg("loopback");
        if passphrase.is_some() {
            cmd.arg("--passphrase-fd").arg("0");
        }
        cmd.args(args);

        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        {
            let stdin = child
                .stdin
                .as_mut()
                .ok_or_else(|| AcsError::gpg("无法打开 gpg 的 stdin"))?;
            if let Some(pp) = passphrase {
                stdin.write_all(pp.as_bytes())?;
            }
            if let Some(data) = stdin_data {
                stdin.write_all(data)?;
            }
        }

        let out = child.wait_with_output()?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(AcsError::gpg(err));
        }
        Ok((
            String::from_utf8_lossy(&out.stdout).into(),
            String::from_utf8_lossy(&out.stderr).into(),
        ))
    }

    // ---- 密钥 ----

    /// 删除本机 gpg 密钥对（注销账户时清理本地私钥）。
    /// 先删私钥再删公钥；密钥本已不存在时视为成功（幂等）。
    /// `passphrase` 可选：仅在某些 gpg 版本要求解锁才能删除时用得上。
    pub fn delete_key(&self, fingerprint: &str, passphrase: Option<&str>) -> Result<()> {
        let _ = self.run(&["--delete-secret-keys", fingerprint], passphrase, None);
        let _ = self.run(&["--delete-keys", fingerprint], passphrase, None);
        Ok(())
    }

    /// 按指纹彻底删除公私钥（注册失败残留的重复密钥清理用）。
    /// 返回是否删除成功（gpg 退出码为 0）。
    pub fn delete_key_strict(&self, fingerprint: &str) -> bool {
        self.run(&["--delete-secret-and-public-keys", fingerprint], None, None)
            .is_ok()
    }

    /// 生成 ed25519 密钥对（cert+sign），user_id 形如 `"ID-Type <email>"`。
    /// 返回指纹、armored 公钥、密码上锁的 armored 私钥。
    ///
    /// **自愈**（最多重试一次，重试前先修复环境）：
    /// - `agent_genkey failed: No such file or directory` —— gpg-agent 按 homedir 常驻，
    ///   homedir 曾被删除/替换而 agent 仍在时会出现（重装、迁移数据目录、删除钱包后重建）。
    /// - `Trust DB error` / `invalid record type` —— 信任库损坏（见 `is_trustdb_error`）。
    /// - `already exists` —— 同 user_id 已有密钥或残留陈旧锁，先清理再生成。
    ///
    /// 修复手段依次为：结束**本 homedir** 的 agent → 重建信任库 → 清理陈旧锁/残留临时文件。
    /// 重试仍失败则把 gpg 的原始报错原样抛出（不再包装成笼统文案，便于定位）。
    pub fn generate_key(&self, user_id: &str, passphrase: &str) -> Result<GeneratedKey> {
        fs::create_dir_all(&self.homedir)?;
        let args = ["--quick-generate-key", user_id, "ed25519", "sign"];
        if let Err(e) = self.run(&args, Some(passphrase), None) {
            crate::log::err(format!("gpg 生成密钥失败：{e}；尝试自愈后重试"));
            self.repair_home(&e);
            self.run(&args, Some(passphrase), None).map_err(|e2| {
                crate::log::err(format!(
                    "gpg 生成密钥失败（自愈后仍失败）：{e2}（首次错误：{e}）"
                ));
                e2
            })?;
        }
        let fingerprint = self.fingerprint(user_id)?;
        let pubkey = self.export_public_key(&fingerprint)?;
        let secret = self.export_secret_key(&fingerprint, passphrase)?;
        Ok(GeneratedKey {
            fingerprint,
            pubkey,
            encrypted_seckey: secret,
        })
    }

    /// 针对一次失败的生成操作做环境修复（尽量温和：能不动就不动）。
    /// 返回是否做了实质修复，便于日志与测试断言。
    pub fn repair_home(&self, err: &AcsError) -> bool {
        let msg = err.to_string();
        let mut repaired = false;
        if is_stale_agent_error(err) {
            self.kill_agent();
            repaired = true;
        }
        if is_trustdb_error(err) {
            repaired |= self.repair_trustdb();
        }
        if msg.contains(ALREADY_EXISTS_HINT) || is_stale_agent_error(err) {
            // 同 UID 重复生成或 agent 残留时，顺手清掉陈旧锁与中断留下的 keybox 临时文件
            repaired |= self.clean_stale_locks();
        }
        if repaired {
            crate::log::info("gpg 环境自愈完成（agent/信任库/陈旧锁）");
        }
        repaired
    }

    /// 结束**本 homedir** 的 gpg-agent。
    ///
    /// 必须精确匹配命令行里的 `--homedir`：Gpg4win 的 `gpgconf --homedir X --kill` 在 2.5.x 上
    /// 会连带杀掉**默认** homedir 的 agent（它按连接名解析 socket），一旦杀错，用户平时用
    /// Kleopatra / git 签名就会受影响。故 Windows 下改为按进程命令行精确筛选后终止。
    fn kill_agent(&self) {
        #[cfg(windows)]
        {
            let marker = format!("--homedir {} ", self.homedir.display());
            let marker_eq = format!("--homedir={}", self.homedir.display());
            let script = format!(
                "Get-CimInstance Win32_Process -Filter \"Name='gpg-agent.exe'\" | \
                 Where-Object {{ $_.CommandLine -and \
                   ($_.CommandLine.Contains('{m}') -or $_.CommandLine.Contains('{e}')) }} | \
                 ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}",
                m = marker.replace('\'', "''"),
                e = marker_eq.replace('\'', "''"),
            );
            let mut ps = Command::new("powershell");
            hide_child_window(&mut ps); // 不要弹黑窗（v3.3.0 #12）
            let _ = ps
                .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        // 非 Windows（或上面的精确终止没生效）时回退到 gpgconf，并显式把 stdio 置空：
        // 不能继承本进程 stdout/stderr —— 否则 gpg 的输出可能被写进它正在改写的文件。
        let conf = self
            .gpg_path
            .with_file_name(if cfg!(windows) { "gpgconf.exe" } else { "gpgconf" });
        let mut gc = Command::new(conf);
        hide_child_window(&mut gc); // 不要弹黑窗（v3.3.0 #12）
        let _ = gc
            .arg("--homedir")
            .arg(&self.homedir)
            .args(["--kill", "gpg-agent"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .stdin(Stdio::null())
            .status();
    }

    /// 重建损坏的 trustdb：**不触碰任何密钥材料**。
    ///
    /// 损坏文件先转存为 `trustdb.gpg.corrupt-<时间戳>`（留证），删除后 gpg 会自动重建
    /// （本机自有密钥本为 ultimate 信任，重建不丢信息）；GnuPG 2.5+ 对空/缺失信任库
    /// 是惰性重建的（直接删即可），旧版则用 `--check-trustdb` 显式触发。
    /// 返回是否真的修复（未检测到损坏则返回 false，不误删健康文件）。
    pub fn repair_trustdb(&self) -> bool {
        let trustdb = self.homedir.join("trustdb.gpg");
        if !trustdb.exists() {
            return false;
        }
        // 只在「确实读不出信任库」时才动它：能正常导出 ownertrust 就说明没坏
        if self
            .run(&["--export-ownertrust"], None, None)
            .is_ok()
        {
            return false;
        }
        self.kill_agent();
        let stamp = chrono::Utc::now().timestamp();
        let backup = self.homedir.join(format!("trustdb.gpg.corrupt-{stamp}"));
        let renamed = fs::rename(&trustdb, &backup).is_ok();
        if !renamed {
            // 改名失败（被占用）时退一步：直接删除，保证可用性优先
            let _ = fs::remove_file(&trustdb);
        }
        let _ = fs::remove_file(self.homedir.join("trustdb.gpg.lock"));
        // 触发重建；失败也不在这里报错，交给调用方的重试结果说话
        let _ = self.run(&["--check-trustdb"], None, None);
        let _ = self.run(&["--list-keys"], None, None);
        crate::log::info(format!(
            "已重建损坏的信任库：{}（备份：{}）",
            trustdb.display(),
            if renamed {
                backup.display().to_string()
            } else {
                "改名失败，已直接删除".to_string()
            }
        ));
        true
    }

    /// 清理陈旧锁文件与中断残留的 keybox 临时文件（`pubring.kb_`）。
    /// 仅删除**超过 `STALE_LOCK_SECS` 未更新**的锁，避免误删正在使用的锁。
    /// 返回是否有清理动作。
    pub fn clean_stale_locks(&self) -> bool {
        let mut cleaned = false;
        let Ok(entries) = fs::read_dir(&self.homedir) else {
            return false;
        };
        let now = std::time::SystemTime::now();
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let is_orphan_keybox_tmp = name == "pubring.kb_";
            if !is_orphan_keybox_tmp && !is_lock_file(&path) {
                continue;
            }
            if !is_orphan_keybox_tmp {
                let too_young = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| now.duration_since(t).ok())
                    .map(|d| d.as_secs() < STALE_LOCK_SECS)
                    .unwrap_or(false);
                if too_young {
                    continue;
                }
            }
            if fs::remove_file(&path).is_ok() {
                crate::log::info(format!("已清理 gpg 陈旧文件：{name}"));
                cleaned = true;
            }
        }
        cleaned
    }

    /// 按 User ID 查指纹。
    ///
    /// 信任库损坏时 `--list-keys` 会直接失败（`Trust DB error`），导致登录 / 签名 / 注册
    /// 全线不可用，故这里也做一次自愈：修复后重试一次，仍失败则原样抛出 gpg 报错。
    pub fn fingerprint(&self, uid: &str) -> Result<String> {
        let args = ["--with-colons", "--list-keys", uid];
        let (out, _) = match self.run(&args, None, None) {
            Ok(v) => v,
            Err(e) if is_trustdb_error(&e) => {
                crate::log::err(format!("gpg 列举密钥失败（疑似信任库损坏）：{e}"));
                self.repair_trustdb();
                self.run(&args, None, None)?
            }
            Err(e) => return Err(e),
        };
        Self::parse_fingerprint(&out, uid)
    }

    /// 从 armored 公钥解析指纹（隔离临时 homedir，供中心/服务端校验注册公钥）。
    pub fn fingerprint_of_armored_pubkey(&self, pubkey_armored: &str) -> Result<String> {
        let dir = self.tmp_dir()?;
        let homedir = dir.join("homedir");
        fs::create_dir_all(&homedir)?;
        self.run_homedir(&homedir, &["--import"], None, Some(pubkey_armored.as_bytes()))?;
        let (out, _) = self.run_homedir(&homedir, &["--with-colons", "--list-keys"], None, None)?;
        let _ = fs::remove_dir_all(&dir);
        Self::parse_fingerprint(&out, "pubkey")
    }

    fn parse_fingerprint(out: &str, uid: &str) -> Result<String> {
        // 同 UID 可能存在多把密钥（如 root 重置后重建），取最后一条 = 最新生成的密钥。
        let mut last: Option<String> = None;
        for line in out.lines() {
            if line.starts_with("fpr:") {
                let parts: Vec<&str> = line.split(':').collect();
                if parts.len() > 9 && !parts[9].is_empty() {
                    last = Some(parts[9].to_string());
                }
            }
        }
        last.ok_or_else(|| AcsError::gpg(format!("未找到 {} 的指纹", uid)))
    }

    /// 导出公钥（armored）。
    pub fn export_public_key(&self, fingerprint: &str) -> Result<String> {
        let (out, _) = self.run(&["--armor", "--export", fingerprint], None, None)?;
        Ok(out)
    }

    /// 导出密码上锁的私钥（armored，GnuPG 可读取）。
    pub fn export_secret_key(&self, fingerprint: &str, passphrase: &str) -> Result<String> {
        let (out, _) = self.run(
            &["--armor", "--export-secret-keys", fingerprint],
            Some(passphrase),
            None,
        )?;
        Ok(out)
    }

    /// 导入 armored 密钥（客户端恢复本地密钥时用）。
    pub fn import_key(&self, armored: &str) -> Result<()> {
        fs::create_dir_all(&self.homedir)?;
        self.run(&["--import"], None, Some(armored.as_bytes()))?;
        Ok(())
    }

    // ---- 签名 ----

    /// 对数据做 detached 签名（armored）。同时校验密码是否正确。
    pub fn sign_detached(
        &self,
        fingerprint: &str,
        passphrase: &str,
        data: &[u8],
    ) -> Result<String> {
        let dir = self.tmp_dir()?;
        let data_path = dir.join("data.bin");
        let sig_path = dir.join("sig.asc");
        fs::write(&data_path, data)?;
        let r = self.run(
            &[
                "--detach-sign",
                "--armor",
                "--local-user",
                fingerprint,
                "--output",
                sig_path.to_str().unwrap(),
                data_path.to_str().unwrap(),
            ],
            Some(passphrase),
            None,
        );
        if r.is_err() {
            let _ = fs::remove_dir_all(&dir);
            return r.map(|_| String::new());
        }
        let sig = fs::read_to_string(&sig_path)?;
        let _ = fs::remove_dir_all(&dir);
        Ok(sig)
    }

    /// 校验密码能否解开私钥（登录探测）。
    /// 失败时把 gpg 的原始报错带上去（口令正确与否由调用方按文案判断，
    /// 但绝不能把真实原因吞成空白，否则登录失败无从排查）。
    pub fn verify_passphrase(&self, fingerprint: &str, passphrase: &str) -> Result<()> {
        self.sign_detached(fingerprint, passphrase, b"acs-passphrase-check")
            .map_err(|e| AcsError::gpg(format!("无法用该口令解锁私钥：{e}")))?;
        Ok(())
    }

    /// 无需口令的「身份签名」：私钥*公开*存在且**未被口令保护**时，gpg 可直接签名。
    ///
    /// 用途：只读查询的持有性证明（如待收箱的 nonce 签名）——这类接口不需要「用户当场
    /// 授权」，只需证明「请求方持有该账户私钥」，故不应强迫用户输口令。
    /// 本项目的密钥一律由口令保护（见 `generate_key`），因此本函数只对未加密密钥有效；
    /// 失败时由调用方回退到「需要口令」的路径。
    pub fn sign_detached_public_key(&self, fingerprint: &str, data: &[u8]) -> Result<String> {
        let dir = self.tmp_dir()?;
        let data_path = dir.join("data.bin");
        let sig_path = dir.join("sig.asc");
        fs::write(&data_path, data)?;
        // 不传 --passphrase-fd：让 gpg 走 agent；口令保护的密钥会失败（预期）
        let r = self.run(
            &[
                "--detach-sign",
                "--armor",
                "--local-user",
                fingerprint,
                "--output",
                sig_path.to_str().unwrap(),
                data_path.to_str().unwrap(),
            ],
            None,
            None,
        );
        let out = match r {
            Ok(_) => fs::read_to_string(&sig_path).map_err(AcsError::from),
            Err(e) => Err(e),
        };
        let _ = fs::remove_dir_all(&dir);
        out
    }

    /// 用给定公钥验证 detached 签名（隔离临时 homedir，不污染用户钥环）。
    pub fn verify_detached(
        &self,
        pubkey_armored: &str,
        data: &[u8],
        sig_armored: &str,
    ) -> Result<bool> {
        let dir = self.tmp_dir()?;
        let homedir = dir.join("homedir");
        fs::create_dir_all(&homedir)?;

        // 去掉首尾空白，避免 armored 块因换行/空格解析失败
        let pubkey_trimmed = pubkey_armored.trim();
        let sig_trimmed = sig_armored.trim();
        if pubkey_trimmed.is_empty() || sig_trimmed.is_empty() {
            let _ = fs::remove_dir_all(&dir);
            return Ok(false);
        }

        self.run_homedir(
            &homedir,
            &["--import"],
            None,
            Some(pubkey_trimmed.as_bytes()),
        )?;

        let data_path = dir.join("data.bin");
        let sig_path = dir.join("sig.asc");
        fs::write(&data_path, data)?;
        fs::write(&sig_path, sig_trimmed)?;

        let r = self.run_homedir(
            &homedir,
            &["--verify", sig_path.to_str().unwrap(), data_path.to_str().unwrap()],
            None,
            None,
        );
        let _ = fs::remove_dir_all(&dir);
        Ok(r.is_ok())
    }

    // ---- 工具 ----

    fn tmp_dir(&self) -> Result<PathBuf> {
        let dir = std::env::temp_dir().join(format!("acs-gpg-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir)?;
        Ok(dir)
    }
}

#[cfg(test)]
mod tests {
    //! 需要真实 GnuPG 的集成测试：默认 `#[ignore]`，按需运行：
    //! `cargo test -p acs-core --lib gpg:: -- --ignored --nocapture`
    use super::*;

    fn gpg_path() -> Option<PathBuf> {
        crate::gpg_detect::find_gpg()
    }

    fn util(homedir: &Path) -> Option<GpgUtil> {
        Some(GpgUtil::new(gpg_path()?, homedir))
    }

    fn tmp_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("acs-gpgtest-{tag}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 复现并覆盖现场故障：信任库损坏 → 密钥生成失败；自愈后**密钥不丢**、生成恢复可用。
    ///
    /// 现场样本（用户真实文件）头部形如 `\x01gpg\x03\x03\x01\x05…` —— gpg 的日志文本被写进了
    /// trustdb.gpg，之后 `--quick-generate-key` 报 `invalid record type` / `Trust DB error`。
    /// 这里用同形状的损坏文件复现，并显式走 `repair_home`（自愈入口）。
    #[test]
    #[ignore = "需要本机安装 GnuPG"]
    fn trustdb_corruption_self_heals_without_losing_keys() {
        let Some(g) = util(&tmp_home("trustdb")) else {
            eprintln!("跳过：本机未找到 gpg");
            return;
        };
        let pass = "Test-Pass-1234!";
        let uid = "RepairProbe-Ind <probe@example.test>";

        // 1) 先有一把正常密钥
        let key = g.generate_key(uid, pass).unwrap();
        assert!(!key.fingerprint.is_empty());

        // 2) 复刻现场：信任库不可读。现场症状是 gpg 的日志文本被写进 trustdb.gpg，
        //    导致任何读取都报 `invalid record type` / `Trust DB error`；这里用
        //    「同名目录占位」把不可读状态做得确定可复现（gpg 无法把它当文件读）。
        let trustdb = g.homedir.join("trustdb.gpg");
        fs::remove_file(&trustdb).unwrap();
        fs::create_dir(&trustdb).unwrap();
        fs::write(trustdb.join("junk"), b"gpg: invalid record type 103 at recnum 1").unwrap();

        // 3) 自愈入口：给一个与现场一致的信任库错误
        let err = AcsError::gpg(
            "gpg: trustdb.gpg: invalid record type 103 at recnum 1\n\
             gpg: Error: The trustdb is corrupted.",
        );
        assert!(g.repair_home(&err), "应识别信任库损坏并完成修复");

        // 4) 不可读的信任库已被移走，并留证备份
        assert!(!trustdb.is_dir(), "占位的损坏信任库应被移走以便重建");
        assert!(trustdb.is_file(), "应已重建出新的信任库");
        let backup = fs::read_dir(&g.homedir)
            .unwrap()
            .flatten()
            .find(|e| e.file_name().to_string_lossy().starts_with("trustdb.gpg.corrupt-"))
            .expect("损坏的信任库应转存为 trustdb.gpg.corrupt-<ts> 留证");
        assert!(backup.path().is_dir(), "备份应保留原始（此处为占位目录）形态");

        // 5) 密钥没丢：原密钥仍在，且能重新列出指纹
        let fpr_after = g.fingerprint(uid).unwrap();
        assert_eq!(fpr_after, key.fingerprint, "重建信任库不得丢失已有密钥");

        // 6) 生成新密钥也恢复正常
        let key2 = g.generate_key("RepairProbe2-Ind <probe2@example.test>", pass).unwrap();
        assert_ne!(key2.fingerprint, key.fingerprint);

        g.kill_agent();
        let _ = fs::remove_dir_all(&g.homedir);
    }

    /// `fingerprint()` 自身也必须在信任库损坏时自愈（登录 / 签名路径全靠它）。
    #[test]
    #[ignore = "需要本机安装 GnuPG"]
    fn fingerprint_survives_corrupt_trustdb() {
        let Some(g) = util(&tmp_home("fpr")) else {
            eprintln!("跳过：本机未找到 gpg");
            return;
        };
        let uid = "FprProbe-Ind <fpr@example.test>";
        let key = g.generate_key(uid, "Test-Pass-1234!").unwrap();

        // 损坏信任库后直接调用 fingerprint()：必须自愈并返回同一指纹
        let trustdb = g.homedir.join("trustdb.gpg");
        let mut junk = vec![0x01u8, b'g', b'p', b'g', 0x03, 0x03, 0x01, 0x05];
        junk.extend(std::iter::repeat_n(0x41u8, 64));
        fs::write(&trustdb, &junk).unwrap();

        assert_eq!(g.fingerprint(uid).unwrap(), key.fingerprint);

        g.kill_agent();
        let _ = fs::remove_dir_all(&g.homedir);
    }

    /// 健康信任库不得被误删（避免自愈逻辑变成数据破坏）。
    #[test]
    #[ignore = "需要本机安装 GnuPG"]
    fn repair_trustdb_is_noop_on_healthy_home() {
        let Some(g) = util(&tmp_home("healthy")) else {
            eprintln!("跳过：本机未找到 gpg");
            return;
        };
        g.generate_key("HealthyProbe-Ind <healthy@example.test>", "Test-Pass-1234!")
            .unwrap();
        assert!(!g.repair_trustdb(), "健康信任库不应被判定为损坏");
        assert!(g.homedir.join("trustdb.gpg").exists(), "健康信任库必须原样保留");
        g.kill_agent();
        let _ = fs::remove_dir_all(&g.homedir);
    }

    /// 陈旧锁与中断残留的 keybox 临时文件应被清理；新鲜锁必须保留。
    #[test]
    #[ignore = "需要本机安装 GnuPG"]
    fn clean_stale_locks_keeps_fresh_locks() {
        let Some(g) = util(&tmp_home("locks")) else {
            eprintln!("跳过：本机未找到 gpg");
            return;
        };
        fs::create_dir_all(&g.homedir).unwrap();
        let stale = g.homedir.join("pubring.kbx.lock");
        let fresh = g.homedir.join("trustdb.gpg.lock");
        let orphan = g.homedir.join("pubring.kb_");
        fs::write(&stale, b"").unwrap();
        fs::write(&fresh, b"").unwrap();
        fs::write(&orphan, b"").unwrap();
        // 把 stale 的修改时间改到 5 分钟前
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(300);
        filetime_set(&stale, old);

        assert!(g.clean_stale_locks());
        assert!(!stale.exists(), "超过 60 秒的锁应被清理");
        assert!(fresh.exists(), "新鲜锁不得被清理（可能正在使用）");
        assert!(!orphan.exists(), "中断残留的 pubring.kb_ 应被清理");
        let _ = fs::remove_dir_all(&g.homedir);
    }

    /// 设置文件修改时间（仅测试用）。断言真的生效，避免「设置了但没生效」导致假失败。
    fn filetime_set(path: &Path, t: std::time::SystemTime) {
        let secs = t
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        #[cfg(windows)]
        {
            // 用 (Get-Date '1970-01-01').AddSeconds() 而非 [datetime]::UnixEpoch：
            // 后者在 Windows PowerShell 5.1（.NET Framework）上不存在，会静默失败。
            let script = format!(
                "$f=Get-Item -LiteralPath '{}'; $f.LastWriteTime=(Get-Date '1970-01-01').AddSeconds({secs}).ToLocalTime()",
                path.display()
            );
            let _ = std::process::Command::new("powershell")
                .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        #[cfg(not(windows))]
        {
            let _ = std::process::Command::new("touch")
                .args(["-d", &format!("@{secs}"), &path.display().to_string()])
                .status();
        }
        let got = fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(u64::MAX);
        assert!(
            got.abs_diff(secs) <= 5,
            "测试前置失败：未能把 {} 的修改时间改到 {secs}（当前 {got}）",
            path.display()
        );
    }
}
