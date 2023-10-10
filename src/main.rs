#![warn(clippy::pedantic)]
#![allow(clippy::wildcard_imports, clippy::too_many_lines)]

use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::MAIN_SEPARATOR;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{self, Poll};

use anyhow::Context;
use axum::body::{self, Bytes, HttpBody};
use axum::http::header::CONTENT_TYPE;
use axum::middleware::map_response;
use axum::response::{Html, IntoResponse, Response};
use axum::{routing, Router, Server};
use cargo_config2::DocConfig;
use cargo_metadata::{Metadata, MetadataCommand};
use clap::{Parser, ValueEnum};
use log::{debug, info};
use tokio::select;
use tower_http::services::ServeDir;
use tower_livereload::LiveReloadLayer;
use watchexec::action::{Action, Outcome};
use watchexec::command::Command;
use watchexec::config::{InitConfig, RuntimeConfig};
use watchexec::event::ProcessEnd;
use watchexec::Watchexec;
use watchexec_filterer_globset::GlobsetFilterer;

type Result<T = (), E = anyhow::Error> = std::result::Result<T, E>;

#[derive(Parser, Debug)]
#[command(bin_name = "cargo")]
enum Cli {
    Watchdoc {
        /// Opens docs in webbrowser
        ///
        /// If a package is specied it is opened, otherwise the root package is
        /// selected.
        #[arg(
            short, long, num_args = 0..=1,
            default_missing_value = "crate",
            value_name = "PACKAGE",
        )]
        open: Option<String>,
        /// Clears terminal between runs
        #[arg(short, long)]
        clear: bool,
        /// Forces theme
        #[arg(short, long)]
        theme: Option<Theme>,
        /// Arguments after `--` are passed to `cargo doc`
        #[arg(allow_hyphen_values = true, last = true)]
        cargo_doc_args: Vec<String>,
    },
}

#[derive(ValueEnum, Clone, Debug, Copy)]
enum Theme {
    Light,
    Dark,
    Ayu,
    AutoAyu,
    AutoDark,
}

#[tokio::main]
async fn main() -> Result {
    run().await
}

async fn run() -> Result {
    let Cli::Watchdoc {
        open,
        cargo_doc_args,
        clear,
        theme,
    } = Cli::parse();

    let cargo_config2::Config {
        doc: DocConfig { browser, .. },
        ..
    } = cargo_config2::Config::load()?;

    stderrlog::new().init()?;

    let ref metadata @ Metadata {
        ref target_directory,
        ref workspace_root,
        ..
    } = MetadataCommand::new().exec()?;

    let command = Command::Exec {
        prog: "cargo".into(),
        args: ["doc".into()].into_iter().chain(cargo_doc_args).collect(),
    };
    command.to_spawnable()?.status().await?;

    let livereload = LiveReloadLayer::new();
    let reloader = livereload.reloader();
    let mut app = Router::new().nest_service(
        "/",
        ServeDir::new(target_directory.join("doc")).not_found_service(routing::get(|| async {
            Html(r#"404 <a href="/help.html?search=">Back to Search</a>"#)
        })),
    );
    if let Some(theme) = theme {
        app = app.layer(map_response(move |response: Response| async move {
            inject_theme_setter(response, theme)
        }));
    }
    let app = app.layer(livereload);

    let port = if portpicker::is_free(4153) {
        4153
    } else {
        portpicker::pick_unused_port().expect("there should be an unsused port left")
    };
    let addr = &SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into();
    let app = Server::bind(addr).serve(app.into_make_service());

    let root = open
        .as_deref()
        .and_then(|o| (o != "crate").then_some(o))
        .or_else(|| {
            Some(
                &metadata
                    .root_package()
                    .or_else(|| metadata.workspace_packages().first().copied())?
                    .name,
            )
        })
        .context("Project must have either a root package or workspace members")?
        .replace('-', "_");

    let addr = format!("http://{addr}/{root}");
    eprintln!("Serving docs at: {addr}");
    if open.is_some() {
        if let Some(browser) = browser {
            std::process::Command::new(browser.path)
                .args(browser.args)
                .arg(addr)
                .spawn()?;
        } else {
            opener::open(addr)?;
        }
    }

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

    let we = Watchexec::new(
        InitConfig::default(),
        RuntimeConfig::default()
            .pathset([workspace_root])
            .command(command)
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
                    debug!("Handle actions: {action:?}");
                    if action.events.iter().any(|e| e.signals().next().is_some()) {
                        action.outcome(Outcome::Exit);
                        Ok::<_, std::convert::Infallible>(())
                    } else {
                        if action.events.iter().any(|e| {
                            info!("Reloading Docs");
                            e.completions()
                                .any(|c| c.is_some_and(|c| c == ProcessEnd::Success))
                        }) {
                            reloader.reload();
                        }
                        if action.events.iter().any(|e| e.paths().next().is_some()) {
                            action.outcome(Outcome::both(
                                Outcome::Wait,
                                Outcome::both(
                                    if clear {
                                        Outcome::Clear
                                    } else {
                                        Outcome::DoNothing
                                    },
                                    Outcome::Start,
                                ),
                            ));
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

fn inject_theme_setter(response: Response, theme: Theme) -> impl IntoResponse {
    use axum::Error;
    struct InjectBody(body::BoxBody, Option<&'static str>);
    impl HttpBody for InjectBody {
        type Data = Bytes;
        type Error = Error;

        fn poll_data(
            mut self: std::pin::Pin<&mut Self>,
            cx: &mut task::Context<'_>,
        ) -> Poll<Option<Result<Self::Data, Self::Error>>> {
            let poll = Pin::new(&mut self.0).poll_data(cx);
            match poll {
                Poll::Ready(None) => Poll::Ready(
                    self.1
                        .take()
                        .map(|theme| Ok(Bytes::from_static(theme.as_bytes()))),
                ),
                poll => poll,
            }
        }

        fn poll_trailers(
            mut self: Pin<&mut Self>,
            cx: &mut task::Context<'_>,
        ) -> Poll<Result<Option<axum::http::HeaderMap>, Self::Error>> {
            Pin::new(&mut self.0).poll_trailers(cx)
        }
    }

    macro_rules! theme_injection {
        ($dark:literal, $theme:literal, $auto:literal) => {
            concat!(
                r#"<script/> updateLocalStorage("preferred-dark-theme", ""#,
                $dark,
                r#""); updateLocalStorage("theme", ""#,
                $theme,
                r#""); updateLocalStorage("use-system-theme", ""#,
                $auto,
                r#""); updateTheme() </script>"#
            )
        };
    }

    let theme = match theme {
        Theme::Light => theme_injection!("dark", "light", "false"),
        Theme::Dark => theme_injection!("dark", "dark", "false"),
        Theme::Ayu => theme_injection!("ayu", "ayu", "false"),
        Theme::AutoAyu => theme_injection!("ayu", "ayu", "true"),
        Theme::AutoDark => theme_injection!("dark", "dark", "true"),
    };

    if response
        .headers()
        .get(CONTENT_TYPE)
        .is_some_and(|ct| ct.to_str().is_ok_and(|ct| ct.starts_with("text/html")))
    {
        let (parts, body) = response.into_parts();
        Response::from_parts(parts, body::boxed(InjectBody(body, Some(theme))))
    } else {
        response
    }
}
