//! 极简 tar 归档读写（v3.3.0 #13 / D36：AlphaTar 服务包 = tar + gzip [+ 可选加密]）。
//!
//! 为什么自己实现：只需要「把目录打成 tar、从 tar 还原目录」这一件事，
//! 引入 `tar` crate 会增加依赖面；USTAR 格式本身很简单，自实现也便于精确控制路径安全
//! （拒绝 `..`、绝对路径等 tar-slip 攻击）。
//!
//! 支持：普通文件 + 目录 + 长路径（USTAR prefix/name 组合，≤255 字节）。
//! 不支持：符号链接、硬链接、稀疏文件、>8GB 单文件（本项目的服务目录用不到）。

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};

const BLOCK: usize = 512;

/// 归档中的一项。
#[derive(Debug, Clone)]
pub struct Entry {
    /// 相对路径（始终用 `/` 分隔，不含前导 `./` 或 `/`）。
    pub path: String,
    pub is_dir: bool,
    pub data: Vec<u8>,
}

/// 把目录递归打成 tar 字节流；`top` 作为**包内顶层目录名**（通常是服务域名）。
///
/// 例：`pack_dir(dir, "acsystem.maxshin.top")` → 包内条目 `acsystem.maxshin.top/.env` 等。
/// 顶层目录名是导入方识别「这是哪个服务」与解包落点的依据。
pub fn pack_dir(root: &Path, top: &str) -> Result<Vec<u8>> {
    let root = root
        .canonicalize()
        .with_context(|| format!("目录不存在：{}", root.display()))?;
    let top = top.trim().trim_matches('/');
    if top.is_empty()
        || top.contains('/')
        || top.contains('\\')
        || top.contains("..")
        || top.contains(':')
    {
        bail!("非法顶层目录名：{top}");
    }
    let mut entries: Vec<Entry> =
        vec![Entry { path: top.to_string(), is_dir: true, data: Vec::new() }];
    let mut children: Vec<Entry> = Vec::new();
    walk(&root, &root, &mut children)?;
    for mut e in children {
        e.path = format!("{top}/{}", e.path);
        entries.push(e);
    }
    // 按路径排序：包内容稳定，便于比对与测试
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut out = Vec::new();
    for e in &entries {
        write_entry(&mut out, e)?;
    }
    // 结束：两个全零块
    out.extend(std::iter::repeat(0u8).take(BLOCK * 2));
    Ok(out)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<Entry>) -> Result<()> {
    let mut items: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("读取目录失败：{}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    items.sort();
    for p in items {
        let rel = match p.strip_prefix(root) {
            Ok(r) => r,
            Err(_) => continue,
        };
        let rel_s = rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        if rel_s.is_empty() {
            continue;
        }
        let md = fs::symlink_metadata(&p).with_context(|| format!("读取元数据失败：{}", p.display()))?;
        if md.is_dir() {
            out.push(Entry { path: rel_s, is_dir: true, data: Vec::new() });
            walk(root, &p, out)?;
        } else if md.is_file() {
            // 跳过 SQLite 的 -wal / -shm（导出的服务包只需一致的 db 快照；
            // 直接复制 wal 可能与主库不一致，反而损坏）
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with("-wal") || name.ends_with("-shm") {
                continue;
            }
            let data = fs::read(&p).with_context(|| format!("读取文件失败：{}", p.display()))?;
            out.push(Entry { path: rel_s, is_dir: false, data });
        }
        // 其他类型（符号链接等）忽略
    }
    Ok(())
}

fn write_entry(out: &mut Vec<u8>, e: &Entry) -> Result<()> {
    let mut header = [0u8; BLOCK];
    // name / prefix：优先整段放 name（≤100 字节），否则切分到 prefix
    let (name, prefix) = split_path(&e.path);
    if name.len() > 100 {
        bail!("路径过长，无法写入 tar：{}", e.path);
    }
    header[..name.len()].copy_from_slice(name.as_bytes());
    if let Some(p) = &prefix {
        if p.len() > 155 {
            bail!("路径前缀过长，无法写入 tar：{}", e.path);
        }
        header[345..345 + p.len()].copy_from_slice(p.as_bytes());
    }
    write_octal(&mut header[100..108], 0o644); // mode
    write_octal(&mut header[108..116], 0); // uid
    write_octal(&mut header[116..124], 0); // gid
    let size = if e.is_dir { 0 } else { e.data.len() as u64 };
    write_octal(&mut header[124..136], size);
    let mtime = chrono::Utc::now().timestamp().max(0) as u64;
    write_octal(&mut header[136..148], mtime);
    header[156] = if e.is_dir { b'5' } else { b'0' };
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    // 校验和：先把 chksum 区当空格，累加后写入
    header[148..156].copy_from_slice(b"        ");
    let sum: u32 = header.iter().map(|b| *b as u32).sum();
    let chk = format!("{:06o}\0 ", sum);
    header[148..156].copy_from_slice(chk.as_bytes());
    out.extend_from_slice(&header);
    if !e.is_dir {
        out.extend_from_slice(&e.data);
        let pad = (BLOCK - (e.data.len() % BLOCK)) % BLOCK;
        out.extend(std::iter::repeat(0u8).take(pad));
    }
    Ok(())
}

/// 把相对路径拆成 (name ≤100B, prefix ≤155B)。
fn split_path(path: &str) -> (String, Option<String>) {
    if path.len() <= 100 {
        return (path.to_string(), None);
    }
    // 从后往前找可切割的 `/`，使后半 ≤100、前半 ≤155
    let bytes = path.as_bytes();
    let mut cut = None;
    for i in (1..bytes.len()).rev() {
        if bytes[i] == b'/' && bytes.len() - i - 1 <= 100 {
            if i <= 155 {
                cut = Some(i);
                break;
            }
        }
    }
    match cut {
        Some(i) => (path[i + 1..].to_string(), Some(path[..i].to_string())),
        None => (path.to_string(), None),
    }
}

fn write_octal(field: &mut [u8], value: u64) {
    // 8 字节字段：7 位八进制 + NUL；12 字节字段：11 位八进制 + NUL
    let width = field.len();
    let s = format!("{:0width$o}", value, width = width - 1);
    let s = if s.len() > width - 1 { &s[s.len() - (width - 1)..] } else { &s };
    field[..s.len()].copy_from_slice(s.as_bytes());
    field[s.len()] = 0;
}

/// 解析 tar 字节流（用于导入）。
pub fn unpack(data: &[u8]) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut off = 0usize;
    while off + BLOCK <= data.len() {
        let header = &data[off..off + BLOCK];
        if header.iter().all(|b| *b == 0) {
            break; // 结束块
        }
        let name = read_str(&header[0..100]);
        let prefix = read_str(&header[345..500]);
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let size = read_octal(&header[124..136]);
        let typeflag = header[156];
        off += BLOCK;
        let is_dir = typeflag == b'5';
        let mut body = Vec::new();
        if !is_dir && size > 0 {
            let end = off
                .checked_add(size as usize)
                .filter(|e| *e <= data.len())
                .ok_or_else(|| anyhow::anyhow!("tar 数据被截断"))?;
            body.extend_from_slice(&data[off..end]);
            off = end;
        }
        // 跳过数据区填充
        let pad = (BLOCK - (size as usize % BLOCK)) % BLOCK;
        off = (off + pad).min(data.len());
        if path.is_empty() {
            continue;
        }
        validate_rel_path(&path)?;
        entries.push(Entry { path, is_dir, data: body });
    }
    if entries.is_empty() {
        bail!("压缩包为空或不是有效的 tar");
    }
    Ok(entries)
}

/// 拒绝绝对路径 / `..` / Windows 盘符，防止 tar-slip 写到目标目录之外。
fn validate_rel_path(p: &str) -> Result<()> {
    if p.starts_with('/') || p.starts_with('\\') || p.contains(':') {
        bail!("压缩包内存在非法绝对路径：{p}");
    }
    for seg in p.split('/') {
        if seg == ".." {
            bail!("压缩包内存在非法路径（..）：{p}");
        }
    }
    Ok(())
}

/// 把条目解包到目标目录（目标目录需已存在）。
pub fn extract_to(entries: &[Entry], dest: &Path) -> Result<u64> {
    fs::create_dir_all(dest)?;
    let mut bytes = 0u64;
    for e in entries {
        let target = dest.join(e.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        // 二次防御：确认解析后的路径仍在 dest 下
        let norm = norm_path(&target)?;
        if !norm.starts_with(&norm_path(dest)?) {
            bail!("拒绝解包到目标目录之外：{}", target.display());
        }
        if e.is_dir {
            fs::create_dir_all(&norm)?;
        } else {
            if let Some(parent) = norm.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&norm, &e.data)?;
            bytes += e.data.len() as u64;
        }
    }
    Ok(bytes)
}

fn norm_path(p: &Path) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => out.push(s),
            Component::ParentDir => {
                if !out.pop() {
                    bail!("路径越界");
                }
            }
            Component::RootDir | Component::Prefix(_) => out.push(c.as_os_str()),
            Component::CurDir => {}
        }
    }
    Ok(out)
}

fn read_str(field: &[u8]) -> String {
    let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).trim().to_string()
}

fn read_octal(field: &[u8]) -> u64 {
    let s = read_str(field);
    u64::from_str_radix(s.trim(), 8).unwrap_or(0)
}

/// 读取 tar 中每个文件的字节数（供测试/展示）。
#[allow(dead_code)]
pub fn total_bytes(entries: &[Entry]) -> u64 {
    entries.iter().map(|e| e.data.len() as u64).sum()
}

/// 按顶层目录名分组（导入时用于识别包内的 `<域名>/` 根目录）。
pub fn top_level(entries: &[Entry]) -> Vec<String> {
    let mut set = HashMap::new();
    for e in entries {
        if let Some(top) = e.path.split('/').next() {
            set.insert(top.to_string(), true);
        }
    }
    let mut v: Vec<String> = set.into_keys().collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_dir() {
        let root = std::env::temp_dir().join(format!("acs-tar-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.env"), b"KEY=1").unwrap();
        fs::write(root.join("sub/b.log"), b"hello").unwrap();
        fs::write(root.join("sub/skip-wal"), b"wal").unwrap();
        // 模拟 sqlite wal 跳过规则（-wal 结尾）
        fs::write(root.join("x.db-wal"), b"wal").unwrap();

        let tar = pack_dir(&root, "pkg").unwrap();
        let entries = unpack(&tar).unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert!(names.contains(&"pkg/a.env"));
        assert!(names.contains(&"pkg/sub/b.log"));
        assert!(!names.iter().any(|n| n.ends_with("-wal")), "sqlite wal 应被跳过");

        let dest = root.join("out");
        extract_to(&entries, &dest).unwrap();
        assert_eq!(fs::read(dest.join("pkg/a.env")).unwrap(), b"KEY=1");
        assert_eq!(fs::read(dest.join("pkg/sub/b.log")).unwrap(), b"hello");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_tar_slip() {
        let mut buf = Vec::new();
        // 构造一个恶意条目 ../evil
        let mut header = [0u8; BLOCK];
        header[..11].copy_from_slice(b"../evil.txt");
        header[124..136].copy_from_slice(b"00000000003\0");
        header[156] = b'0';
        let sum: u32 = header.iter().map(|b| *b as u32).sum();
        let chk = format!("{:06o}\0 ", sum);
        header[148..156].copy_from_slice(chk.as_bytes());
        buf.extend_from_slice(&header);
        buf.extend_from_slice(b"bad");
        let pad = (BLOCK - (3 % BLOCK)) % BLOCK;
        buf.extend(std::iter::repeat(0u8).take(pad));
        buf.extend(std::iter::repeat(0u8).take(BLOCK * 2));
        let err = unpack(&buf).unwrap_err();
        assert!(err.to_string().contains(".."));
    }
}
