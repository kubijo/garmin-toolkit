use std::future::Future;
use std::num::NonZeroU64;
use std::time::Duration;

use garmin_gfdi::pairing::PairingSession;
use garmin_gfdi::{DeviceInformation, RouteKind, select_route};
use tokio::time::{Instant, MissedTickBehavior};

use super::{Capture, LiveOptions, SmokeError, Transport, bluez, cancelled, close_owned, send};

/// Establish a `BlueZ` bond and complete Garmin companion setup on the exact selected device.
/// Uses the desktop's default Bluetooth agent for user confirmation. Never removes bonds,
/// changes trust, scans, or transfers files. A failed setup can leave the new OS bond in place.
///
/// # Errors
/// Rejects connected targets, unsupported protocol, failed acknowledgements, deadlines and cleanup failures.
pub async fn pair(options: LiveOptions) -> Result<DeviceInformation, SmokeError> {
    if !(Duration::from_secs(1)..=Duration::from_secs(600)).contains(&options.timeout) {
        return Err(SmokeError::Preconditions(
            "pairing timeout must be between one and 600 seconds",
        ));
    }
    let mut capture = Capture::create_download(&options.capture_dir)?;
    capture.metadata(
        "pair_target",
        &serde_json::json!({"adapter": options.adapter, "device": options.device.to_string()}),
    )?;
    let deadline = Instant::now() + options.timeout;
    let mut transport = tokio::time::timeout_at(
        deadline,
        bluez::BluezTransport::open_for_pairing(&options.adapter, options.device),
    )
    .await
    .map_err(|_| SmokeError::Timeout)??;
    execute_pair(
        &mut transport,
        &mut capture,
        &options,
        deadline,
        cancelled(),
    )
    .await
}

async fn execute_pair<T: Transport>(
    transport: &mut T,
    capture: &mut Capture,
    options: &LiveOptions,
    deadline: Instant,
    cancellation: impl Future<Output = Result<(), SmokeError>> + Send,
) -> Result<DeviceInformation, SmokeError> {
    let mut engine = None;
    let mut attempted = false;
    let result = tokio::select! {
        biased;
        signal = cancellation => signal.and(Err(SmokeError::Cancelled)),
        result = tokio::time::timeout_at(deadline, observe(transport, capture, options, deadline, &mut engine, &mut attempted)) => result.unwrap_or(Err(SmokeError::Timeout)),
    };
    let mut failures = Vec::new();
    if let Some(close) = engine.as_ref().and_then(PairingSession::close_request)
        && let Err(error) = tokio::time::timeout(
            Duration::from_secs(2),
            close_owned(transport, capture, close),
        )
        .await
        .unwrap_or(Err(SmokeError::Timeout))
    {
        failures.push(error.to_string());
    }
    if attempted
        && let Err(error) = tokio::time::timeout(Duration::from_secs(3), transport.disconnect())
            .await
            .unwrap_or(Err(SmokeError::Timeout))
    {
        failures.push(error.to_string());
    }
    let outcome = result.as_ref().map_or_else(ToString::to_string, |_| {
        "Garmin setup acknowledged".to_owned()
    });
    if let Err(error) = capture
        .metadata(
            "pair_result",
            &serde_json::json!({"outcome": outcome, "cleanup_errors": failures}),
        )
        .and_then(|()| capture.finish())
    {
        failures.push(error.to_string());
    }
    if failures.is_empty() {
        result
    } else {
        Err(SmokeError::Cleanup {
            observation: outcome,
            cleanup: failures.join("; "),
        })
    }
}

async fn observe<T: Transport>(
    transport: &mut T,
    capture: &mut Capture,
    options: &LiveOptions,
    deadline: Instant,
    engine: &mut Option<PairingSession>,
    attempted: &mut bool,
) -> Result<DeviceInformation, SmokeError> {
    *attempted = true;
    eprintln!("Bluetooth pairing: establishing the OS bond");
    transport.bond().await?;
    capture.record("bonded", None)?;
    transport.connect().await?;
    let route = select_route(&transport.inventory().await?).ok_or(SmokeError::NoRoute)?;
    if route.kind != RouteKind::MultiLink {
        return Err(SmokeError::Preconditions(
            "Garmin pairing currently requires MultiLink",
        ));
    }
    transport.subscribe(route).await?;
    let random = uuid::Uuid::new_v4().as_u128().to_le_bytes();
    let client =
        NonZeroU64::new(u64::from_le_bytes(random[..8].try_into().map_err(
            |_| SmokeError::Preconditions("client ID generation failed"),
        )?))
        .ok_or(SmokeError::Preconditions("zero client ID"))?;
    let start = Instant::now();
    let (session, request) = PairingSession::begin(
        client,
        options.mode,
        deadline.saturating_duration_since(start),
    );
    *engine = Some(session);
    send(transport, capture, "register", &request).await?;
    let engine = engine
        .as_mut()
        .ok_or(SmokeError::Preconditions("pairing engine missing"))?;
    eprintln!("Bluetooth bonded; completing Garmin setup");
    let mut timer = tokio::time::interval(Duration::from_millis(5));
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut last_progress = Duration::ZERO;
    let mut phase = "";
    loop {
        let outgoing = tokio::select! {
            packet = transport.next() => {
                let packet = packet?.ok_or(SmokeError::Disconnected)?;
                capture.record("receive", Some(&packet))?;
                engine.receive(&packet, start.elapsed())?
            }
            _ = timer.tick() => engine.poll(start.elapsed())?,
        };
        for packet in outgoing {
            send(transport, capture, "send", &packet).await?;
        }
        if phase != engine.progress()
            || start.elapsed().saturating_sub(last_progress) >= Duration::from_secs(1)
        {
            phase = engine.progress();
            eprintln!(
                "Bluetooth pairing: {phase} ({} s)",
                start.elapsed().as_secs()
            );
            last_progress = start.elapsed();
        }
        if let Some(identity) = engine.completed() {
            return Ok(identity.clone());
        }
    }
}
