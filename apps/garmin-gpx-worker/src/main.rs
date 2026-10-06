//! Private, resource-limited parser helper for native hosts.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    garmin_gpx::worker::run_worker()
}
