use anyhow::Result;
use serde::Serialize;
use std::fs;
use std::path::Path;

#[derive(Serialize)]
struct DaemonConfig<'a> {
    daemon: DaemonSection,
    cache_targets: Vec<CacheTarget>,
    paths: Vec<DaemonPath<'a>>,
}

#[derive(Serialize)]
struct DaemonSection {
    http_port: u16,
}

#[derive(Serialize)]
struct CacheTarget {
    url: String,
}

#[derive(Serialize)]
struct DaemonPath<'a> {
    path: String,
    alias: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<&'a str>,
}

#[derive(Serialize)]
struct ServeConfig<'a> {
    me: ServeMe,
    paths: Vec<ServePath<'a>>,
}

#[derive(Serialize)]
struct ServeMe {
    ip: &'static str,
}

#[derive(Serialize)]
struct ServePath<'a> {
    alias: &'a str,
    ip: &'static str,
    port: u16,
}

pub fn write(
    home: &Path,
    dest: &Path,
    port: u16,
    alias: &str,
    language: Option<&str>,
) -> Result<()> {
    fs::create_dir_all(home)?;
    let path = std::path::absolute(dest)?
        .to_string_lossy()
        .replace('\\', "/");
    let daemon = DaemonConfig {
        daemon: DaemonSection { http_port: 0 },
        cache_targets: vec![CacheTarget {
            url: format!("http://127.0.0.1:{port}"),
        }],
        paths: vec![DaemonPath {
            path,
            alias,
            language,
        }],
    };
    let serve = ServeConfig {
        me: ServeMe { ip: "127.0.0.1" },
        paths: vec![ServePath {
            alias,
            ip: "127.0.0.1",
            port,
        }],
    };
    fs::write(
        home.join("daemon.toml"),
        format!(
            "# Конфигурация копии проекта\n{}",
            toml::to_string(&daemon)?
        ),
    )?;
    fs::write(
        home.join("serve.toml"),
        format!("# Конфигурация копии проекта\n{}", toml::to_string(&serve)?),
    )?;
    Ok(())
}
