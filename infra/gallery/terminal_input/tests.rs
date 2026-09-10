use super::*;
use gallery::{
    Gallery, Manifest, Renderer, SceneCtx, SceneEntry, SceneGroupMeta, SceneSource, Settings,
};
use garmin_cli_tui::{
    RunProfile,
    preview::{PreviewScreen, render_preview},
};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

const CELLS: [u16; 2] = [80, 24];

fn render(input: &mut TerminalInput) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(CELLS[0], CELLS[1])).expect("terminal");
    terminal
        .draw(|frame| {
            render_preview(
                frame,
                RunProfile::PRODUCTION,
                PreviewScreen::ProgressOverflow,
                10,
                &mut input.state,
            );
        })
        .expect("preview");
    terminal.backend().buffer().clone()
}

fn row_below(buffer: &Buffer, text: &str) -> u16 {
    (0..buffer.area.height)
        .find(|row| {
            (0..buffer.area.width)
                .filter_map(|column| buffer.cell((column, *row)))
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
                .contains(text)
        })
        .map(|row| row + 1)
        .expect("panel title")
}

#[derive(Clone)]
struct PreviewFixture {
    inputs: [TerminalInput; 2],
    images: [egui::Rect; 2],
    scene: usize,
}

impl Default for PreviewFixture {
    fn default() -> Self {
        Self {
            inputs: std::array::from_fn(|_| TerminalInput::default()),
            images: [egui::Rect::NOTHING; 2],
            scene: 0,
        }
    }
}

fn fixture_id() -> egui::Id {
    egui::Id::new("terminal-input-fixture")
}

fn previews(ui: &mut Ui, scene: usize) {
    let mut fixture = ui.data_mut(|data| {
        data.get_temp_mut_or_default::<PreviewFixture>(fixture_id())
            .clone()
    });
    fixture.scene = scene;
    let (_, area) = ui.allocate_space(egui::vec2(1240.0, 500.0));
    fixture.images = [
        egui::Rect::from_min_size(area.min, egui::vec2(400.0, 240.0)),
        egui::Rect::from_min_size(area.min + egui::vec2(440.0, 20.0), egui::vec2(800.0, 480.0)),
    ];
    for (index, input) in fixture.inputs.iter_mut().enumerate() {
        render(input);
        let response = ui.interact(
            fixture.images[index],
            egui::Id::new((scene, index)),
            egui::Sense::click(),
        );
        input.interact(ui, &response, fixture.images[index], CELLS);
    }
    ui.data_mut(|data| data.insert_temp(fixture_id(), fixture));
}

struct PreviewSource;

impl SceneSource for PreviewSource {
    fn manifest(&mut self) -> Manifest {
        Manifest {
            scenes: vec![
                SceneEntry {
                    render: |_: &mut SceneCtx<'_>, ui| previews(ui, 0),
                    name: "Progress",
                    module_path: "input",
                    default: true,
                    order: 0,
                    source: "",
                },
                SceneEntry {
                    render: |_: &mut SceneCtx<'_>, ui| previews(ui, 1),
                    name: "Other",
                    module_path: "input",
                    default: false,
                    order: 1,
                    source: "",
                },
            ],
            groups: vec![SceneGroupMeta {
                module_path: "input",
                title: "Input",
            }],
        }
    }
}

struct Harness {
    shell: egui_kittest::Harness<'static, Gallery<PreviewSource>>,
    fixture: PreviewFixture,
}

impl Harness {
    fn new() -> Self {
        let mut harness = Self {
            shell: egui_kittest::Harness::builder()
                .with_size(egui::vec2(1920.0, 1080.0))
                .build_eframe(|cc| {
                    garmin_ui::install_assets(&cc.egui_ctx);
                    Gallery::new(
                        PreviewSource,
                        Settings::new(Renderer::Wgpu).collapsed(false),
                        None,
                        None,
                    )
                }),
            fixture: PreviewFixture::default(),
        };
        harness.frame(Vec::new());
        harness
    }

    fn frame(&mut self, events: Vec<egui::Event>) {
        self.shell
            .ctx
            .data_mut(|data| data.insert_temp(fixture_id(), self.fixture.clone()));
        self.shell.input_mut().events.extend(events);
        self.shell.step();
        self.fixture = self
            .shell
            .ctx
            .data(|data| data.get_temp(fixture_id()))
            .expect("preview fixture");
    }

    fn point(&self, index: usize, cell: [u16; 2]) -> egui::Pos2 {
        let image = self.fixture.images[index];
        image.min
            + egui::vec2(
                (f32::from(cell[0]) + 0.5) / f32::from(CELLS[0]) * image.width(),
                (f32::from(cell[1]) + 0.5) / f32::from(CELLS[1]) * image.height(),
            )
    }

    fn click(&mut self, index: usize, cell: [u16; 2]) {
        let pos = self.point(index, cell);
        self.frame(vec![egui::Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            self.frame(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }]);
        }
        self.frame(Vec::new());
    }

    fn key(&mut self, key: Key) {
        self.key_modifiers(key, egui::Modifiers::NONE);
    }

    fn key_modifiers(&mut self, key: Key, modifiers: egui::Modifiers) {
        self.frame(vec![
            egui::Event::ModifiersChanged(modifiers),
            egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            },
        ]);
        self.frame(vec![
            egui::Event::Key {
                key,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers,
            },
            egui::Event::ModifiersChanged(egui::Modifiers::NONE),
        ]);
    }

    fn wheel(&mut self, index: usize, cell: [u16; 2]) {
        self.frame(vec![egui::Event::PointerMoved(self.point(index, cell))]);
        self.frame(vec![egui::Event::MouseWheel {
            phase: egui::TouchPhase::Move,
            unit: egui::MouseWheelUnit::Point,
            delta: egui::vec2(
                0.0,
                -self.fixture.images[index].height() / f32::from(CELLS[1]) * 2.0,
            ),
            modifiers: egui::Modifiers::NONE,
        }]);
        for _ in 0..30 {
            self.frame(Vec::new());
        }
    }
}

#[test]
fn scaled_wheel_input_changes_only_the_hovered_terminal() {
    for index in 0..2 {
        let mut harness = Harness::new();
        let before: [_; 2] = std::array::from_fn(|i| render(&mut harness.fixture.inputs[i]));
        let row = row_below(&before[index], "History");
        harness.wheel(index, [10, row]);
        assert_ne!(render(&mut harness.fixture.inputs[index]), before[index]);
        assert_eq!(
            render(&mut harness.fixture.inputs[1 - index]),
            before[1 - index]
        );
        assert!(harness.fixture.inputs[index].revision > 0);
        assert_eq!(harness.fixture.inputs[1 - index].revision, 0);
    }
}

#[test]
fn focused_terminal_claims_navigation_before_the_gallery_shell() {
    let mut harness = Harness::new();
    let before = render(&mut harness.fixture.inputs[0]);
    harness.key(Key::End);
    assert_eq!(render(&mut harness.fixture.inputs[0]), before);
    harness.click(0, [10, 11]);
    harness.key(Key::End);
    let active_scrolled = render(&mut harness.fixture.inputs[0]);
    assert_ne!(active_scrolled, before);
    harness.key(Key::Tab);
    assert_eq!(harness.fixture.scene, 0);
    let history_focused = render(&mut harness.fixture.inputs[0]);
    harness.key(Key::End);
    let history_scrolled = render(&mut harness.fixture.inputs[0]);
    assert_ne!(history_scrolled, history_focused);
    harness.key_modifiers(Key::Tab, egui::Modifiers::SHIFT);
    assert_eq!(harness.fixture.scene, 0);
    harness.key(Key::Home);
    assert_ne!(render(&mut harness.fixture.inputs[0]), history_scrolled);
    let before_release = render(&mut harness.fixture.inputs[0]);
    harness.key(Key::Escape);
    harness.key(Key::Home);
    assert_eq!(render(&mut harness.fixture.inputs[0]), before_release);
    assert!(
        !harness
            .shell
            .ctx
            .memory(|memory| memory.has_focus(egui::Id::new((0_usize, 0_usize))))
    );
    harness.key(Key::Tab);
    assert_eq!(harness.fixture.scene, 1);
    harness.key_modifiers(Key::Tab, egui::Modifiers::SHIFT);
    assert_eq!(harness.fixture.scene, 0);
}
