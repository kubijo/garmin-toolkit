//! Dedicated native activity-map tile worker.

use std::{
    path::Path,
    sync::{Arc, mpsc},
    thread::JoinHandle,
    time::Duration,
};

use garmin_ui::activity::{MapTileDecoder, MapTileRequest, MapTileResponse};

const MAX_IN_FLIGHT: usize = 6;

pub struct Worker {
    commands: mpsc::Sender<Command>,
    responses: mpsc::Receiver<Response>,
    join: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn spawn(cache_root: &Path, context: eframe::egui::Context) -> std::io::Result<Self> {
        let service = garmin_map_tiles::Service::new(cache_root).map_err(std::io::Error::other)?;
        let (commands, command_receiver) = mpsc::channel();
        let (response_sender, responses) = mpsc::channel();
        let join = std::thread::Builder::new()
            .name("garmin-toolkit-map-tiles".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("activity map runtime must start");
                let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_IN_FLIGHT));
                let decoder = Arc::new(MapTileDecoder::default());
                while let Ok(command) = command_receiver.recv() {
                    let Command::Tile(target, request) = command else {
                        break;
                    };
                    let service = service.clone();
                    let response_sender = response_sender.clone();
                    let context = context.clone();
                    let semaphore = Arc::clone(&semaphore);
                    let decoder = Arc::clone(&decoder);
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
                        let _ignored = response_sender.send(Response {
                            target,
                            tile: decoder.decode(request, result),
                        });
                        context.request_repaint();
                    }));
                }
                runtime.shutdown_timeout(Duration::from_secs(1));
            })?;
        Ok(Self {
            commands,
            responses,
            join: Some(join),
        })
    }

    pub fn request(&self, target: Target, request: MapTileRequest) {
        let _ignored = self.commands.send(Command::Tile(target, request));
    }

    pub fn drain(&self) -> impl Iterator<Item = Response> + '_ {
        self.responses.try_iter()
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
    Tile(Target, MapTileRequest),
    Shutdown,
}

#[derive(Clone, Copy)]
pub enum Target {
    Activity,
    FitPreview,
}

pub struct Response {
    pub target: Target,
    pub tile: MapTileResponse,
}
