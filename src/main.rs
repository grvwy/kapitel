#![forbid(unsafe_code)]
use kapitel::{Config, auth};
use std::io::Read;

fn main() -> anyhow::Result<()> {
    rustix::process::umask(rustix::fs::Mode::RWXG | rustix::fs::Mode::RWXO);
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().is_some_and(|value| value == "inspect") {
        use rustix::process::{Resource, Rlimit, setrlimit};
        setrlimit(
            Resource::As,
            Rlimit {
                current: Some(384 * 1024 * 1024),
                maximum: Some(384 * 1024 * 1024),
            },
        )?;
        setrlimit(
            Resource::Cpu,
            Rlimit {
                current: Some(20),
                maximum: Some(20),
            },
        )?;
        let extension = arguments
            .get(1)
            .ok_or_else(|| anyhow::anyhow!("Missing format"))?;
        let file = std::fs::File::open("/proc/self/fd/0")?;
        println!(
            "{}",
            serde_json::to_string(&kapitel::metadata::inspect(file, extension)?)?
        );
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "kapitel=info".into()),
        )
        .with_target(false)
        .init();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(async_main(arguments))
}

async fn async_main(arguments: Vec<String>) -> anyhow::Result<()> {
    let config = Config::from_environment()?;
    let state = kapitel::initialize(config).await?;
    match arguments.first().map(String::as_str).unwrap_or("serve") {
        "serve" => {
            let router = kapitel::router(state.clone()).await?;
            tokio::spawn(kapitel::index::run(state));
            let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
            tracing::info!("Kapitel is listening on port 8080");
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let mut terminate =
                        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                            .expect("SIGTERM handler");
                    tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
                })
                .await?;
        }
        "user" => {
            let action = arguments.get(1).map(String::as_str).unwrap_or("");
            let username = arguments.get(2).ok_or_else(|| {
                anyhow::anyhow!(
                    "Usage: kapitel user create|reset USERNAME [--admin] [--password-stdin]"
                )
            })?;
            let password = if arguments.iter().any(|value| value == "--password-stdin") {
                let mut password = String::new();
                std::io::stdin().take(1024).read_to_string(&mut password)?;
                password.trim_end_matches(['\r', '\n']).to_string()
            } else {
                let password = rpassword::prompt_password("Password (at least 15 characters): ")?;
                let confirmation = rpassword::prompt_password("Confirm password: ")?;
                anyhow::ensure!(password == confirmation, "Passwords do not match");
                password
            };
            if action == "create" {
                auth::insert_profile(
                    &state,
                    auth::NewProfile {
                        username: username.clone(),
                        password,
                        is_admin: arguments.iter().any(|value| value == "--admin"),
                    },
                )
                .await
                .map_err(|error| anyhow::anyhow!(error.2))?;
            } else if action == "reset" {
                let id: String = sqlx::query_scalar("SELECT id FROM profiles WHERE username=?")
                    .bind(username)
                    .fetch_one(&state.database)
                    .await?;
                auth::change_profile(
                    &state,
                    &id,
                    auth::ProfileUpdate {
                        enabled: None,
                        password: Some(password),
                    },
                )
                .await
                .map_err(|error| anyhow::anyhow!(error.2))?;
            } else {
                anyhow::bail!("Use user create or user reset");
            }
            println!("Profile updated");
        }
        _ => anyhow::bail!(
            "Usage: kapitel [serve | user create USERNAME --admin | user reset USERNAME]"
        ),
    }
    Ok(())
}
