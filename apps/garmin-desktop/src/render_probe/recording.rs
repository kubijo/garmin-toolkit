//! Bounded recording queue. PNG compression and file I/O never run in the frame loop.

use std::{fs, io, sync::mpsc, thread};
use std::{
    fs::File,
    io::Write as _,
    path::Path,
    sync::mpsc::{SyncSender, TrySendError},
    thread::JoinHandle,
};

use eframe::egui;
use serde::Serialize;

use super::Result;

struct Capture {
    frame: u64,
    seconds: f64,
    pixels: egui::ColorImage,
}

#[derive(Default, Serialize)]
pub(super) struct Summary {
    queued: u64,
    dropped: u64,
}

pub(super) struct Recording {
    sender: SyncSender<Capture>,
    worker: JoinHandle<io::Result<()>>,
    summary: Summary,
    maximum: u64,
}

impl Recording {
    pub fn new(output: &Path, maximum: u64) -> Result<Self> {
        let directory = output.join("frames");
        fs::create_dir(&directory)?;
        let index = File::create_new(output.join("frames.jsonl"))?;
        let (sender, receiver) = mpsc::sync_channel(2);
        let worker = thread::Builder::new()
            .name("probe-recording".into())
            .spawn(move || write_frames(&directory, index, &receiver))?;
        Ok(Self {
            sender,
            worker,
            summary: Summary::default(),
            maximum,
        })
    }

    pub fn has_capacity(&self) -> bool {
        self.summary.queued < self.maximum
    }

    pub fn offer(&mut self, frame: u64, seconds: f64, pixels: egui::ColorImage) -> Result<()> {
        if !self.has_capacity() {
            self.summary.dropped += 1;
            return Ok(());
        }
        match self.sender.try_send(Capture {
            frame,
            seconds,
            pixels,
        }) {
            Ok(()) => self.summary.queued += 1,
            Err(TrySendError::Full(_)) => self.summary.dropped += 1,
            Err(TrySendError::Disconnected(_)) => {
                return Err(io::Error::other(
                    "frame writer stopped; inspect recording_error in summary",
                )
                .into());
            }
        }
        Ok(())
    }

    pub fn finish(self) -> Result<Summary> {
        drop(self.sender);
        self.worker
            .join()
            .map_err(|_| io::Error::other("frame writer panicked"))??;
        Ok(self.summary)
    }
}

fn write_frames(
    directory: &Path,
    mut index: File,
    receiver: &mpsc::Receiver<Capture>,
) -> io::Result<()> {
    for (sequence, capture) in receiver.iter().enumerate() {
        let file = format!("{sequence:06}.png");
        let [width, height] = capture.pixels.size;
        let pixels: Vec<u8> = capture
            .pixels
            .pixels
            .iter()
            .flat_map(egui::Color32::to_srgba_unmultiplied)
            .collect();
        image::save_buffer(
            directory.join(&file),
            &pixels,
            u32::try_from(width).map_err(io::Error::other)?,
            u32::try_from(height).map_err(io::Error::other)?,
            image::ColorType::Rgba8,
        )
        .map_err(io::Error::other)?;
        serde_json::to_writer(
            &mut index,
            &serde_json::json!({
                "file": format!("frames/{file}"), "frame": capture.frame,
                "seconds": capture.seconds, "width": width, "height": height,
            }),
        )?;
        writeln!(index)?;
        index.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_cap_preserves_pixels_and_original_timestamps() -> Result<()> {
        let output = tempfile::tempdir()?;
        let mut recording = Recording::new(output.path(), 1)?;
        let pixels = egui::ColorImage::new([2, 1], vec![egui::Color32::RED, egui::Color32::BLUE]);
        recording.offer(7, 1.25, pixels.clone())?;
        recording.offer(12, 2.5, pixels)?;
        let summary = recording.finish()?;
        assert_eq!(summary.queued, 1);
        assert_eq!(summary.dropped, 1);
        let decoded = image::open(output.path().join("frames/000000.png"))?.to_rgba8();
        assert_eq!(decoded.dimensions(), (2, 1));
        assert_eq!(decoded.as_raw(), &[255, 0, 0, 255, 0, 0, 255, 255]);
        let index: serde_json::Value =
            serde_json::from_slice(&fs::read(output.path().join("frames.jsonl"))?)?;
        assert_eq!(index["frame"], 7);
        assert_eq!(index["seconds"], 1.25);
        assert_eq!(fs::read_dir(output.path().join("frames"))?.count(), 1);
        Ok(())
    }
}
