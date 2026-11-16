#![cfg_attr(windows, windows_subsystem = "windows")]
//! Прототип: список чатов и переписка на Iced с программным рендером. Только для замера памяти и скорости.
use iced::widget::{column, container, row, scrollable, text, text_input, Column};
use iced::{window, Background, Border, Color, Element, Fill, Length, Subscription, Task};
use std::time::Instant;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const WORDS: [&str; 24] = [
    "привет", "как", "дела", "сегодня", "встреча", "перенесли", "на", "завтра", "документы", "готовы", "посмотри", "пожалуйста", "файл",
    "отправил", "в", "общую", "папку", "спасибо", "большое", "hello", "meeting", "report", "ok", "давай",
];
const NAMES: [&str; 12] = ["Анна", "Борис", "Вера", "Глеб", "Дарья", "Егор", "Жанна", "Зоя", "Игорь", "Кира", "Лев", "Мария"];

fn sentence(rng: &mut Rng, words: usize) -> String {
    (0..words).map(|_| WORDS[rng.below(WORDS.len())]).collect::<Vec<_>>().join(" ")
}

fn rss_kb() -> (u64, u64) {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let get = |k: &str| s.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse().ok()).unwrap_or(0);
    (get("VmRSS:"), get("VmHWM:"))
}

struct Chat {
    name: String,
    preview: String,
    time: String,
    unread: u32,
    color: Color,
}
struct Msg {
    text: String,
    time: String,
    out: bool,
    author: &'static str,
}

struct State {
    chats: Vec<Chat>,
    msgs: Vec<Msg>,
    bench: bool,
    frame: u32,
    t0: Instant,
    input: String,
}

#[derive(Debug, Clone)]
enum Message {
    Frame(Instant),
    Input(String),
}

fn chat_scroll_id() -> iced::widget::Id {
    iced::widget::Id::new("chats")
}
fn msg_scroll_id() -> iced::widget::Id {
    iced::widget::Id::new("msgs")
}

impl State {
    fn new() -> Self {
        let args: Vec<String> = std::env::args().collect();
        let num = |flag: &str, default: usize| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(default);
        let (n_chats, n_msgs) = (num("--chats", 5000), num("--msgs", 200));
        let mut rng = Rng(0x1234_5678_9abc_def1);
        let chats = (0..n_chats)
            .map(|i| {
                let name = format!("{} {}", NAMES[rng.below(NAMES.len())], i);
                let n = 3 + rng.below(8);
                Chat {
                    name,
                    preview: sentence(&mut rng, n),
                    time: format!("{:02}:{:02}", rng.below(24), rng.below(60)),
                    unread: if rng.below(4) == 0 { rng.below(30) as u32 } else { 0 },
                    color: Color::from_rgb8(60 + rng.below(150) as u8, 60 + rng.below(150) as u8, 60 + rng.below(150) as u8),
                }
            })
            .collect();
        let msgs = (0..n_msgs)
            .map(|i| {
                let long = rng.below(6) == 0;
                let n = 2 + rng.below(if long { 70 } else { 14 });
                Msg { text: sentence(&mut rng, n), time: format!("{:02}:{:02}", (i / 60) % 24, i % 60), out: rng.below(3) == 0, author: NAMES[rng.below(NAMES.len())] }
            })
            .collect();
        println!("rss before show {:?} kB", rss_kb());
        State { chats, msgs, bench: args.iter().any(|a| a == "--bench"), frame: 0, t0: Instant::now(), input: String::new() }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Input(s) => {
                self.input = s;
                Task::none()
            }
            Message::Frame(_) => {
                if !self.bench {
                    return Task::none();
                }
                self.frame += 1;
                let tick = self.frame;
                if tick == 1 || tick == 100 || tick == 400 || tick == 800 {
                    println!("tick {tick}: rss/hwm {:?} kB, {:?} elapsed", rss_kb(), self.t0.elapsed());
                }
                if tick >= 800 {
                    println!("done: {:?} for {tick} frames, final rss/hwm {:?} kB", self.t0.elapsed(), rss_kb());
                    return iced::exit();
                }
                let phase = (tick % 400) as f32 / 400.0;
                let tri = if phase < 0.5 { phase * 2.0 } else { 2.0 - phase * 2.0 };
                let chat_y = 64.0 * self.chats.len() as f32 * tri * 0.98;
                let msg_y = 60.0 * self.msgs.len() as f32 * tri * 0.9;
                Task::batch([
                    iced::widget::operation::scroll_to(chat_scroll_id(), scrollable::AbsoluteOffset { x: Some(0.0), y: Some(chat_y) }),
                    iced::widget::operation::scroll_to(msg_scroll_id(), scrollable::AbsoluteOffset { x: Some(0.0), y: Some(msg_y) }),
                ])
            }
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let chats = Column::with_children(self.chats.iter().map(|c| {
            let avatar = container(text(c.name.chars().next().unwrap_or('?').to_string()).size(18).color(Color::WHITE))
                .width(44)
                .height(44)
                .center_x(44)
                .center_y(44)
                .style(move |_| container::Style { background: Some(Background::Color(c.color)), border: Border { radius: 22.0.into(), ..Border::default() }, ..Default::default() });
            let badge: Element<Message> = if c.unread > 0 {
                container(text(c.unread.to_string()).size(10).color(Color::WHITE))
                    .padding([1, 6])
                    .style(|_| container::Style { background: Some(Background::Color(Color::from_rgb8(0x3b, 0x82, 0xc4))), border: Border { radius: 9.0.into(), ..Border::default() }, ..Default::default() })
                    .into()
            } else {
                text("").into()
            };
            container(row![
                avatar,
                column![
                    row![text(&c.name).size(14).color(Color::WHITE).width(Fill), text(&c.time).size(11).color(Color::from_rgb8(0x6d, 0x7f, 0x8f))],
                    row![text(&c.preview).size(12).color(Color::from_rgb8(0x7f, 0x91, 0xa4)).width(Fill).height(16), badge],
                ]
                .spacing(2)
                .width(Fill)
            ]
            .spacing(10))
            .padding(10)
            .height(64)
            .into()
        }));
        let left = container(scrollable(chats).id(chat_scroll_id()).height(Fill))
            .width(320)
            .height(Fill)
            .style(|_| container::Style { background: Some(Background::Color(Color::from_rgb8(0x17, 0x21, 0x2b))), ..Default::default() });
        let msgs = Column::with_children(self.msgs.iter().map(|m| {
            let mut inner = Column::new().spacing(2).padding(8);
            if !m.out {
                inner = inner.push(text(m.author).size(12).color(Color::from_rgb8(0x6a, 0xb3, 0xf3)));
            }
            inner = inner.push(text(&m.text).size(14).color(Color::WHITE));
            inner = inner.push(text(&m.time).size(10).color(Color::from_rgb8(0x8a, 0xa0, 0xb4)).width(Fill).align_x(iced::alignment::Horizontal::Right));
            let bg = if m.out { Color::from_rgb8(0x2b, 0x52, 0x78) } else { Color::from_rgb8(0x18, 0x25, 0x33) };
            let bubble = container(inner).max_width(560).style(move |_| container::Style { background: Some(Background::Color(bg)), border: Border { radius: 12.0.into(), ..Border::default() }, ..Default::default() });
            container(bubble).width(Fill).align_x(if m.out { iced::alignment::Horizontal::Right } else { iced::alignment::Horizontal::Left }).into()
        }))
        .spacing(6)
        .padding(12);
        let right = column![
            container(text("Чат").size(16).color(Color::WHITE)).padding(14).width(Fill).style(|_| container::Style { background: Some(Background::Color(Color::from_rgb8(0x17, 0x21, 0x2b))), ..Default::default() }),
            scrollable(msgs).id(msg_scroll_id()).height(Fill),
            container(text_input("Написать сообщение…", &self.input).on_input(Message::Input)).padding(10).width(Fill),
        ]
        .width(Length::Fill);
        container(row![left, right]).style(|_| container::Style { background: Some(Background::Color(Color::from_rgb8(0x0e, 0x16, 0x21))), ..Default::default() }).into()
    }

    fn subscription(&self) -> Subscription<Message> {
        if self.bench { window::frames().map(Message::Frame) } else { Subscription::none() }
    }
}

fn main() -> iced::Result {
    iced::application(State::new, State::update, State::view).title("Tg-Lite 2.0 prototype: Iced (tiny-skia)").subscription(State::subscription).window_size((1000.0, 650.0)).run()
}
