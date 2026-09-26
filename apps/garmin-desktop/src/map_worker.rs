//! Dedicated native activity-map tile worker.

use std::{
    path::Path,
    sync::{Arc, mpsc},
    thread::JoinHandle,
    time::Duration,
};

use garmin_ui::activity::map_runtime::{Backend, TileCoordinates, TileData, TileReply};

const MAX_IN_FLIGHT: usize = 6;
const MAX_CPU_WORKERS: usize = 4;

pub struct Worker {
    commands: mpsc::Sender<Command>,
    join: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn spawn(cache_root: &Path) -> std::io::Result<Self> {
        let service = garmin_map_tiles::Service::new(cache_root).map_err(std::io::Error::other)?;
        let (commands, command_receiver) = mpsc::channel();
        let join = std::thread::Builder::new()
            .name("garmin-toolkit-map-tiles".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .max_blocking_threads(MAX_CPU_WORKERS)
                    .enable_all()
                    .build()
                    .expect("activity map runtime must start");
                let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_IN_FLIGHT));
                while let Ok(command) = command_receiver.recv() {
                    let Command::Tile { request, reply } = command else {
                        break;
                    };
                    let service = service.clone();
                    let semaphore = Arc::clone(&semaphore);
                    std::mem::drop(runtime.spawn(async move {
                        let Ok(_permit) = semaphore.acquire_owned().await else {
                            return;
                        };
                        let result = service
                            .tile(garmin_map_tiles::TileId {
                                zoom: request.zoom,
                                x: request.x,
                                y: request.y,
                            })
                            .await
                            .map(|tile| tile.bytes)
                            .map_err(|error| error.to_string());
                        let _ignored = tokio::task::spawn_blocking(move || {
                            reply(result.map(TileData::Encoded));
                        })
                        .await;
                    }));
                }
                runtime.shutdown_timeout(Duration::from_secs(1));
            })?;
        Ok(Self {
            commands,
            join: Some(join),
        })
    }
}

impl Backend for Worker {
    fn fetch(&self, request: TileCoordinates, reply: TileReply) {
        if let Err(error) = self.commands.send(Command::Tile { request, reply }) {
            let Command::Tile { reply, .. } = error.0 else {
                return;
            };
            reply(Err("native map runtime is unavailable".to_owned()));
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ignored = self.commands.send(Command::Shutdown);
        if let Some(join) = self.join.take() {
            let _ignored = join.join();
        }
    }
}

enum Command {
    Tile {
        request: TileCoordinates,
        reply: TileReply,
    },
    Shutdown,
}
