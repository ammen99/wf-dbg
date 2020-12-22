mod message;
mod ipc;

use std::sync::Arc;
use async_std::os::unix::net::UnixStream;
use async_std::sync::Mutex;

use futures::executor::block_on;

use iced::{executor, Application, Command, Element, Settings, Color, Point, Size, Slider, Button, Checkbox};
use iced::widget::canvas::{Canvas, Cache};

use iced::Subscription;
use iced::slider;
use iced::button;

use crate::message::Message;
use crate::ipc::WayfireSocketRecipe;

enum Shape {
    // x, y
    FrameBoundary(u64, usize),
    // [l, r], x
    RepaintRegion(u64, u64, usize),
    // c=[x,y]
    Commit(u64, usize),
}

impl Shape {
    fn left(&self) -> u64 {
        *match self {
            Shape::FrameBoundary(l, _) => l,
            Shape::RepaintRegion(l, _, _) => l,
            Shape::Commit(l, _) => l,
        }
    }

    fn right(&self) -> u64 {
        *match self {
            Shape::FrameBoundary(r, _) => r,
            Shape::RepaintRegion(_, r, _) => r,
            Shape::Commit(r, _) => r,
        }
    }
}

// Maximum duration in which to have events in
const MAX_TIME_PERIOD: u64 = 1u64 * 1_000_000_000u64;

// Scale of the visualization (80ms)
const VISUALIZATION_SCALE: f64 = 120f64 * 1_000_000.0;

const PIXELS_PER_SURFACE: u16 = 20;
const PIXELS_MIN_HEIGHT: u16 = 250;

#[derive(Default)]
struct OutputState {
    name: String,
    last_repaint_start: u64,
}

#[derive(Default)]
struct SurfaceState {
    index: u32,
    output_idx: usize,
}

struct RepaintLoop {
    index: f64,
    drawn: Cache,
    shapes: Vec<Shape>,
    pending_shapes: Vec<Shape>,
    outputs: Vec<OutputState>,
    surfaces: Vec<SurfaceState>,
    do_periodic_refresh: bool,
}

struct RepaintLoopApp {
    slider_state: slider::State,
    refresh_btn: button::State,
    socket: Arc<Mutex<UnixStream>>,
    repaint: RepaintLoop,
}

impl RepaintLoopApp {
    fn new() -> Self {
        let socket_path = std::env::var("WAYFIRE_SOCKET").unwrap();
        let socket = block_on(UnixStream::connect(socket_path)).unwrap();

        Self {
            slider_state: slider::State::new(),
            refresh_btn: button::State::new(),
            socket: Arc::new(Mutex::new(socket)),
            repaint: RepaintLoop::new(),
        }
    }
}

impl RepaintLoop {
    fn new() -> Self {
        RepaintLoop {
            drawn: Cache::new(),
            shapes: Vec::new(),
            pending_shapes: Vec::new(),
            outputs: vec![],
            surfaces: vec![],
            index: 0.0,
            do_periodic_refresh: false,
        }
    }

    fn output_idx(&mut self, output: &String) -> usize {
        if let Some(i) = self.outputs.iter().position(|x| output.eq(&x.name)) {
            return i;
        } else {
            self.outputs.push(OutputState {
                name: output.clone(),
                last_repaint_start: 0
            });
            return self.outputs.len() - 1;
        }
    }

    fn surface_idx(&mut self, surface: u32, output: &String) -> usize {
        if let Some(i) = self.surfaces.iter().position(|x| x.index == surface) {
            return i;
        } else {
            let oidx = self.output_idx(output);
            self.surfaces.push(SurfaceState{
                index: surface,
                output_idx: oidx,
            });
            return self.surfaces.len() - 1;
        }
    }

    fn current_pending_window(&self) -> u64 {
        let fst = self.pending_shapes.first().map_or(0, |s| s.left());
        let lst = self.pending_shapes.last().map_or(0, |s| s.right());
        return lst - fst;
    }

    fn handle_message(&mut self, message: Message) {
        match message {
            Message::FrameStart(output, time) => {
                let idx = self.output_idx(&output);
                self.pending_shapes.push(Shape::FrameBoundary(time, idx));
                //println!("{} Frame starting at {}", idx, time);
            }
            Message::FrameRepaint(output, time) => {
                let idx = self.output_idx(&output);
                self.outputs[idx].last_repaint_start = time;
                //println!("{} Frame repainting at {}", output, time);
            }
            Message::FrameRepaintDone(output, time) => {
                let idx = self.output_idx(&output);
                let start = self.outputs[idx].last_repaint_start;
                self.pending_shapes.push(Shape::RepaintRegion(start, time, idx));
                //println!("{} Frame done at {}", output, time);
            }
            Message::SurfaceCommit(surface, output, time) => {
                let idx = self.surface_idx(surface, &output);
                self.pending_shapes.push(Shape::Commit(time, idx));
            }
            Message::Refresh => {
                self.shapes.clear();
                self.pending_shapes.clear();
            }

            // GUI Events
            Message::SliderChanged(idx) => {
                self.index = idx;
                self.drawn.clear();
            }

            Message::PeriodicRefreshChanged(v) => {
                self.do_periodic_refresh = v;
            }
        }

        if self.shapes.is_empty() && self.current_pending_window() >= MAX_TIME_PERIOD {
            std::mem::swap(&mut self.shapes, &mut self.pending_shapes);
            self.drawn.clear();
        }
    }
}

impl Application for RepaintLoopApp {
    type Executor = executor::Default;
    type Message = Message;
    type Flags = ();

    fn new(_flags: ()) -> (Self, Command<Self::Message>) {
        (Self::new(), Command::none())
    }

    fn title(&self) -> String {
        String::from("Wayfire Repaint Loop")
    }

    fn update(&mut self, message: Self::Message) -> Command<Self::Message> {
        self.repaint.handle_message(message);
        Command::none()
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let mut subs = vec![];

        if self.repaint.current_pending_window() < MAX_TIME_PERIOD {
            subs.push(iced_futures::Subscription::from_recipe(
                    WayfireSocketRecipe::new(self.socket.clone())));
        }

        if self.repaint.do_periodic_refresh {
            subs.push(iced_futures::time::every(std::time::Duration::from_secs(3))
                      .map(|_| Message::Refresh));
        }

        Subscription::batch(subs)
    }

    fn view(&mut self) -> Element<Self::Message> {
        let cvs_h = PIXELS_MIN_HEIGHT.max(
            (self.repaint.surfaces.len() as u16) * PIXELS_PER_SURFACE);

        let idx = self.repaint.index;
        let auto_refresh_state = self.repaint.do_periodic_refresh;

        let canvas = Canvas::new(&mut self.repaint)
            .width(iced::Length::Fill)
            .height(iced::Length::Units(cvs_h));

        let slider = Slider::new(&mut self.slider_state,
                                 0.0..=(MAX_TIME_PERIOD as f64) - VISUALIZATION_SCALE,
                                 idx,
                                 Message::SliderChanged)
            .width(iced::Length::Fill);

        let button = Button::new(&mut self.refresh_btn, iced::Text::new("Refresh"))
            .on_press(Message::Refresh)
            .width(iced::Length::Shrink)
            .height(iced::Length::Shrink);

        let auto_refresh = Checkbox::new(
            auto_refresh_state,
            "Refresh every 3 seconds",
            Message::PeriodicRefreshChanged);

        let widgets = iced::Row::new()
            .width(iced::Length::Fill)
            .height(iced::Length::Shrink)
            .push(slider)
            .push(iced::Space::with_width(iced::Length::Units(20)))
            .push(button)
            .push(iced::Space::with_width(iced::Length::Units(20)))
            .push(auto_refresh);

        iced::Column::new()
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
            .padding(20)
            .push(canvas)
            .push(iced::Space::with_height(iced::Length::Units(50)))
            .push(widgets)
            .into()
    }
}

impl iced::canvas::Program<Message> for RepaintLoop {
    fn draw(&self, bounds: iced::Rectangle, _: iced::canvas::Cursor) -> Vec<iced::canvas::Geometry> {
        let clock = self.drawn.draw(bounds.size(), |frame| {
            let shapes = if self.shapes.is_empty() {
                &self.pending_shapes
            } else {
                &self.shapes
            };

            let begin = shapes.first().map_or(0, |s| s.left());

            let output_labels = 150.0;
            let width = frame.width() - output_labels;
            let height = frame.height();
            frame.translate(iced::Vector::new(output_labels, 0.0));

            let left_boundary = self.index;
            let find_x = |x: &u64| {
//                println!("{} {} {}", *x, begin, left_boundary);
                let relative = (*x - begin) as f64 - left_boundary;
                let scale = width as f64;
                return (relative / VISUALIZATION_SCALE * scale) as f32;
            };

            let visible = |x| {
                x >= 0.0 && x <= width
            };

            let cnt_outputs = std::cmp::max(1, self.outputs.len());
            let cnt_surfaces = std::cmp::max(1, self.surfaces.len() + 1);

            let y_per_output = height / (cnt_outputs as f32);
            let y_per_surface = y_per_output / (cnt_surfaces as f32);

            for (idx, output) in self.outputs.iter().enumerate() {
                let text = iced::canvas::Text {
                    color: Color::BLACK,
                    size: 28.0,
                    position: Point::new(-output_labels, ((idx as f32) + 0.5) * y_per_output),
                    content: output.name.clone(),
                    horizontal_alignment: iced::HorizontalAlignment::Left,
                    ..iced::canvas::Text::default()
                };

                frame.fill_text(text);
            }


            let boundary = iced::canvas::Stroke {
                width: 4.0,
                color: Color::BLACK,
                ..iced::canvas::Stroke::default()
            };

            let thin = iced::canvas::Stroke {
                width: 1.0,
                color: Color::from_rgb(0.5, 0.5, 0.5),
                ..iced::canvas::Stroke::default()
            };

            let repaint_rect = iced::canvas::Fill {
                color: Color::from_rgb(0.5, 0.5, 1.0),
                ..iced::canvas::Fill::default()
            };

            let commit_circle = iced::canvas::Fill {
                color: Color::BLACK,
                ..iced::canvas::Fill::default()
            };

            for shape in shapes {
                match shape {
                    Shape::FrameBoundary(x, idx) => {
                        let xp = find_x(x);
                        if !visible(xp) {
                            continue;
                        }
                        let xp = xp.max(1.5);
                        let i = *idx as f32;
                        let path = iced::canvas::Path::line(
                            Point::new(xp, 0.0),
                            Point::new(xp, height - 20.0));
                        frame.stroke(&path, thin);

                        let path = iced::canvas::Path::line(
                            Point::new(xp, y_per_output * i + 5.0),
                            Point::new(xp, y_per_output * (i + 1.0) - 5.0));
                        frame.stroke(&path, boundary);

                        let text = iced::canvas::Text {
                            color: Color::BLACK,
                            size: 14.0,
                            position: Point::new(xp, height),
                            content: format!("{}", (x - begin) / 1_000_000),
                            horizontal_alignment: iced::HorizontalAlignment::Center,
                            ..iced::canvas::Text::default()
                        };

                        frame.fill_text(text);
                    }
                    Shape::RepaintRegion(l, r, idx) => {
                        let lp = find_x(l);
                        let rp = find_x(r);
                        let i = *idx as f32;

                        if !visible(lp) && !visible(rp) {
                            continue;
                        }

                        let path = iced::canvas::Path::rectangle(
                            Point::new(lp, y_per_output * i + 5.0),
                            Size::new(rp - lp, y_per_output - 10.0));

                        frame.fill(&path, repaint_rect);
                    }
                    Shape::Commit(x, idx) => {
                        let xp = find_x(x);
                        let i = *idx as f32;

                        if !visible(xp) {
                            continue;
                        }

                        let yp = self.surfaces[*idx].output_idx as f32 * y_per_output
                            + (i + 1.0) * y_per_surface;

                        let sz = 7.0;
                        let path = iced::canvas::Path::rectangle(
                            Point::new(xp - sz / 2.0, yp - sz / 2.0),
                            Size::new(sz, sz));
                        frame.fill(&path, commit_circle);
                    }
                }
            }
        });

        vec![clock]
    }
}

pub fn main() -> iced::Result {
    RepaintLoopApp::run(Settings::default())
}
//use std::os::unix::net::UnixStream;
//use std::io::prelude::*;
//use std::str::from_utf8;
//
//use serde_json::Value;
//
//fn main() -> std::io::Result<()> {
//    let mut stream = UnixStream::connect("/home/ilex/work/wayfire/build/wayfire.sock")?;
//
//    loop {
//        let mut len_buf = [0; 4]; // Size is u32
//        stream.read_exact(&mut len_buf)?;
//
//        let len = u32::from_ne_bytes(len_buf) as usize;
//
//        println!("Message length: {}", len);
//        let mut message_buf = vec![0u8; len];
//        stream.read_exact(&mut message_buf)?;
//
//        let msg_str = from_utf8(&message_buf).unwrap();
//        let msg: Value = serde_json::from_str(msg_str)?;
//
//        println!("Message is {}", msg);
//    }
//}
