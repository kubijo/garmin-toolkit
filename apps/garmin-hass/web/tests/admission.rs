#[path = "../src/map_worker/admission.rs"]
mod admission;

use admission::{Admission, Step, Work};
use garmin_ui::activity::map_runtime::BrowserTilePacketBuilder;

struct Packet(BrowserTilePacketBuilder);

impl Work for Packet {
    fn retained_bytes(&self) -> usize {
        self.0.retained_bytes()
    }

    fn is_complete(&self) -> bool {
        self.0.next_section().is_none()
    }

    fn advance(&mut self, maximum: usize, _: &mut Vec<u8>) -> Result<Step, String> {
        if self.0.allocate_next()?.is_some() {
            return Ok(Step::Allocated);
        }
        let Some((section, length)) = self.0.next_chunk(maximum) else {
            return Ok(Step::Deferred);
        };
        self.0.append_from(section, length, |bytes| bytes.fill(0))?;
        Ok(Step::Copied {
            bytes: length,
            text_milliseconds: 0.0,
        })
    }
}

#[test]
fn unaligned_strings_do_not_fail_geometry_at_frame_boundaries() {
    for strings in [1, 2, 3, 37, 38, 39] {
        let mut queue = Admission::default();
        let packet =
            Packet(BrowserTilePacketBuilder::new(6 * 1024 * 1024, 12, 0, strings).unwrap());
        assert!(queue.enqueue(packet).is_ok());
        let mut completed = 0;
        for _ in 0..10 {
            if !queue.schedule(false) {
                break;
            }
            let frame = queue.advance_frame(0.0, || 0.0);
            for (packet, result) in frame.completed {
                result.unwrap();
                packet.0.finish().unwrap();
                completed += 1;
            }
        }
        assert_eq!(completed, 1, "string length {strings}");
        assert!(!queue.schedule(false));
    }
}
