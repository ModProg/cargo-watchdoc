#![warn(clippy::pedantic)]
#![allow(clippy::wildcard_imports)]

use std::io;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::MAIN_SEPARATOR;
use std::process::ExitStatus;
use std::sync::Arc;

use anyhow::Context;
use axum::{Router, Server};
use cargo_metadata::{Metadata, MetadataCommand};
use log::{debug, info};
use tokio::select;
use tower_http::services::ServeDir;
use tower_livereload::LiveReloadLayer;
use watchexec::action::{Action, Outcome};
use watchexec::config::{InitConfig, RuntimeConfig};
use watchexec::handler::PrintDebug;
use watchexec::Watchexec;
use watchexec_filterer_globset::GlobsetFilterer;

type Result<T = (), E = anyhow::Error> = std::result::Result<T, E>;

async fn cargo_doc() -> Result<ExitStatus, std::io::Error> {
    info!("running cargo doc");
    tokio::process::Command::new("cargo")
        .arg("doc")
        // .arg("--no-deps")
        .status()
        .await
}

#[tokio::main]
async fn main() -> Result {
    run().await
}

async fn run() -> Result {
    stderrlog::new().init()?;

    let ref metadata @ Metadata {
        ref target_directory,
        ref workspace_root,
        ..
    } = MetadataCommand::new().exec()?;

    cargo_doc().await?;

    let livereload = LiveReloadLayer::new();
    let reloader = livereload.reloader();
    let app = Router::new()
        .nest_service("/", ServeDir::new(target_directory.join("doc")))
        .layer(livereload);

    let port = if portpicker::is_free(4153) {
        4153
    } else {
        portpicker::pick_unused_port().expect("there should be an unsused port left")
    };
    let addr = &SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into();
    let app = Server::bind(addr).serve(app.into_make_service());

    // TODO select either workspace-root package or alphabetically first

    let root = &metadata
        .root_package()
        .or_else(|| metadata.workspace_packages().first().copied())
        .context("Project must have either a root package or workspace members")?
        .name.replace('-', "_");

    eprintln!("Serving docs at: http://{addr}/{root}");

    let list = [
        // Mac
        format!("*{MAIN_SEPARATOR}.DS_Store"),
        // Vim
        "*.sw?".into(),
        "*.sw?x".into(),
        // Emacs
        "#*#".into(),
        ".#*".into(),
        // Kate
        ".*.kate-swp".into(),
        // VCS
        format!("*{MAIN_SEPARATOR}.hg{MAIN_SEPARATOR}**"),
        format!("*{MAIN_SEPARATOR}.git{MAIN_SEPARATOR}**"),
        format!("*{MAIN_SEPARATOR}.svn{MAIN_SEPARATOR}**"),
        // SQLite
        "*.db".into(),
        "*.db-*".into(),
        format!("*{MAIN_SEPARATOR}*.db-journal{MAIN_SEPARATOR}**"),
        // Rust
        format!("*{MAIN_SEPARATOR}target{MAIN_SEPARATOR}**"),
        "rustc-ice-*.txt".into(),
    ];

    debug!("Default ignores: {:?}", list);

    let mut init = InitConfig::default();
    init.on_error(PrintDebug(std::io::stderr()));

    let we = Watchexec::new(
        init,
        RuntimeConfig::default()
            .pathset([workspace_root])
            .filterer(Arc::new(
                GlobsetFilterer::new(
                    &workspace_root,
                    [],
                    list.into_iter().map(|p| (p, None)),
                    ignore_files::from_origin(&workspace_root)
                        .await
                        .0
                        .into_iter()
                        .chain(ignore_files::from_environment(None).await.0),
                    [],
                )
                .await?,
            ))
            .on_action(move |action: Action| {
                let reloader = reloader.clone();
                async move {
                    debug!("{action:?}");
                    if action.events.iter().any(|e| e.signals().next().is_some()) {
                        action.outcome(Outcome::Exit);
                        Ok::<_, io::Error>(())
                    } else {
                        if cargo_doc().await?.success() {
                            reloader.reload();
                        }
                        Ok(())
                    }
                }
            })
            .clone(),
    )?;

    select! {
        we = we.main() => we??,
        app = app => app?
    }

    Ok(())
}
