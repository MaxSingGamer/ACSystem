//! GnuPG（gpg.exe）封装。
//!
//! - 密钥为 ed25519（OpenPGP 格式），与 GnuPG 完全兼容，技术人员可用
//!   `gpg --homedir <~/.alpha_dir/acs-client/gnupg> --list-keys` 直接审查。
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

    /// 生成 ed25519 密钥对（cert+sign），user_id 形如 `"ID-Type <email>"`。
    /// 返回指纹、armored 公钥、密码上锁的 armored 私钥。
    ///
    /// 自愈：gpg-agent 是按 homedir 常驻的，若 homedir 曾被删除/替换而 agent 仍在，
    /// 会报 `agent_genkey failed: No such file or directory`（用户重装、迁移数据目录或
    /// 删除钱包后重建时很常见）。此时杀掉该 homedir 的 agent 再重试一次即可。
    pub fn generate_key(&self, user_id: &str, passphrase: &str) -> Result<GeneratedKey> {
        fs::create_dir_all(&self.homedir)?;
        let args = ["--quick-generate-key", user_id, "ed25519", "sign"];
        if let Err(e) = self.run(&args, Some(passphrase), None) {
            if !is_stale_agent_error(&e) {
                return Err(e);
            }
            self.kill_agent();
            self.run(&args, Some(passphrase), None)?;
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

    /// 结束本 homedir 的 gpg-agent（用于清理已失效的常驻 agent）。
    /// 优先用同目录下的 `gpgconf --homedir <hd> --kill gpg-agent`；失败则忽略（交给重试结果说话）。
    fn kill_agent(&self) {
        let conf = self.gpg_path.with_file_name(if cfg!(windows) { "gpgconf.exe" } else { "gpgconf" });
        let _ = Command::new(conf)
            .arg("--homedir")
            .arg(&self.homedir)
            .args(["--kill", "gpg-agent"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    /// 按 User ID 查指纹。
    pub fn fingerprint(&self, uid: &str) -> Result<String> {
        let (out, _) = self.run(&["--with-colons", "--list-keys", uid], None, None)?;
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
    pub fn verify_passphrase(&self, fingerprint: &str, passphrase: &str) -> Result<()> {
        self.sign_detached(fingerprint, passphrase, b"acs-passphrase-check")?;
        Ok(())
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
