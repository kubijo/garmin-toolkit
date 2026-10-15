use std::{env, net::TcpListener, path::PathBuf, process::Command, time::Duration};

use crate::{
    BrowserOptions,
    devices::{Host, demo::DemoSource},
};

const TEST_ROOT: &str = "GARMIN_STARTUP_TEST_ROOT";
const TEST_CASE: &str = "GARMIN_STARTUP_TEST_CASE";

#[test]
fn process_environment_overrides_startup_defaults() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let web = root.path().join("custom web");
    std::fs::create_dir(&web)?;
    std::fs::write(
        root.path().join("index.html"),
        "wrong current directory frontend",
    )?;
    std::fs::write(
        web.join("index.html"),
        concat!(
            "<html>custom startup frontend ",
            "GARMIN_TOOLKIT_CSP_NONCE ",
            "GARMIN_TOOLKIT_MAP_UPLOAD_TELEMETRY ",
            "GARMIN_TOOLKIT_MAP_RENDER_WORKER ",
            "GARMIN_TOOLKIT_UI_AUTOMATION</html>",
        ),
    )?;
    for case in [
        "configured",
        "allowed-ingress",
        "blocked-ingress",
        "missing-web",
        "invalid-address",
        "invalid-ingress",
        "default-data",
    ] {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        drop(listener);
        let mut command = Command::new(env::current_exe()?);
        command
            .args([
                "--exact",
                "server::startup_tests::environment_worker",
                "--ignored",
                "--nocapture",
            ])
            .current_dir(root.path())
            .env(TEST_ROOT, root.path())
            .env(TEST_CASE, case)
            .env(
                crate::DATA_BASE_ENVIRONMENT,
                root.path().join("data override"),
            )
            .env(super::WEB_ROOT_ENVIRONMENT, &web)
            .env(super::ADDRESS_ENVIRONMENT, address.to_string())
            .env_remove(super::INGRESS_GATEWAY_ENVIRONMENT);
        match case {
            "allowed-ingress" => {
                command.env(super::INGRESS_GATEWAY_ENVIRONMENT, "127.0.0.1");
            }
            "blocked-ingress" => {
                command.env(super::INGRESS_GATEWAY_ENVIRONMENT, "127.0.0.2");
            }
            "missing-web" => {
                command.env(super::WEB_ROOT_ENVIRONMENT, root.path().join("missing"));
            }
            "invalid-address" => {
                command.env(super::ADDRESS_ENVIRONMENT, "invalid");
            }
            "invalid-ingress" => {
                command.env(super::INGRESS_GATEWAY_ENVIRONMENT, "invalid");
            }
            "default-data" => {
                command.env_remove(crate::DATA_BASE_ENVIRONMENT);
            }
            _ => {}
        }
        let result = command.output()?;
        assert!(
            result.status.success(),
            "{case}: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    Ok(())
}

#[test]
#[ignore = "subprocess helper for isolated startup environment acceptance"]
fn environment_worker() -> anyhow::Result<()> {
    let root = PathBuf::from(env::var_os(TEST_ROOT).expect("parent test owns the scratch root"));
    let case = env::var(TEST_CASE)?;
    let base = if case == "default-data" {
        PathBuf::from("/data")
    } else {
        root.join("data override")
    };
    let expected = if cfg!(feature = "demo") {
        base.join("demo")
    } else {
        base
    };
    assert_eq!(crate::deployment_data_root(), expected);
    if case == "default-data" {
        // Resolve the default without opening the user's deployment or touching /data.
        return Ok(());
    }
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let deployment = crate::prepare_deployment(&expected).await?;
        let host = Host::new(
            Box::new(DemoSource::new(
                root.join("device"),
                tokio::runtime::Handle::current(),
            )?),
            std::sync::Arc::clone(&deployment),
        );
        let tiles = garmin_map_tiles::Service::new(root.join("cache"))?;
        let result = if matches!(
            case.as_str(),
            "configured" | "allowed-ingress" | "blocked-ingress"
        ) {
            verify_frontend(host, tiles, case == "blocked-ingress").await
        } else {
            let error = tokio::time::timeout(
                Duration::from_secs(5),
                super::serve(host, tiles, BrowserOptions::default()),
            )
            .await?
            .expect_err("invalid configuration must fail before listening");
            match case.as_str() {
                "missing-web" => assert_eq!(error.kind(), std::io::ErrorKind::NotFound),
                "invalid-address" => assert!(error.to_string().contains("socket address")),
                "invalid-ingress" => assert!(error.to_string().contains("IP address")),
                _ => panic!("unexpected startup case {case}"),
            }
            Ok(())
        };
        deployment.close().await;
        result
    })
}

async fn verify_frontend(
    host: std::sync::Arc<Host>,
    tiles: garmin_map_tiles::Service,
    blocked: bool,
) -> anyhow::Result<()> {
    let address = env::var(super::ADDRESS_ENVIRONMENT)?;
    let server = tokio::spawn(super::serve(host, tiles, BrowserOptions::default()));
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(1))
        .build()?;
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(response) = client.get(format!("http://{address}/")).send().await {
                if blocked {
                    anyhow::ensure!(
                        response.status() == reqwest::StatusCode::FORBIDDEN,
                        "ingress gateway override did not reject the peer"
                    );
                    return Ok::<_, anyhow::Error>(());
                }
                let html = response.error_for_status()?.text().await?;
                anyhow::ensure!(
                    html.contains("custom startup frontend"),
                    "web-root override was not served"
                );
                anyhow::ensure!(
                    !html.contains("GARMIN_TOOLKIT_"),
                    "startup placeholders were not rendered"
                );
                return Ok::<_, anyhow::Error>(());
            }
            anyhow::ensure!(
                !server.is_finished(),
                "server exited before accepting requests"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    server.abort();
    let stopped = server.await.expect_err("the owned test server must stop");
    assert!(stopped.is_cancelled());
    result??;
    Ok(())
}
