#![cfg_attr(windows, windows_subsystem = "windows")]
//! Прототип: список чатов и переписка на Slint с программным рендером. Только для замера памяти и скорости.
use slint::{Color, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::rc::Rc;
use std::time::{Duration, Instant};

slint::include_modules!();

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

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let num = |flag: &str, default: usize| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(default);
    let (n_chats, n_msgs) = (num("--chats", 5000), num("--msgs", 200));
    let bench = args.iter().any(|a| a == "--bench");
    let start = Instant::now();
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let chats: Vec<Chat> = (0..n_chats)
        .map(|i| {
            let name = format!("{} {}", NAMES[rng.below(NAMES.len())], i);
            Chat {
                initial: SharedString::from(name.chars().next().unwrap().to_string()),
                name: name.into(),
                preview: { let n = 3 + rng.below(8); sentence(&mut rng, n) }.into(),
                time: format!("{:02}:{:02}", rng.below(24), rng.below(60)).into(),
                unread: if rng.below(4) == 0 { rng.below(30) as i32 } else { 0 },
                color: Color::from_rgb_u8(60 + rng.below(150) as u8, 60 + rng.below(150) as u8, 60 + rng.below(150) as u8),
            }
        })
        .collect();
    let msgs: Vec<Msg> = (0..n_msgs)
        .map(|i| Msg {
            text: { let long = rng.below(6) == 0; let n = 2 + rng.below(if long { 70 } else { 14 }); sentence(&mut rng, n) }.into(),
            time: format!("{:02}:{:02}", (i / 60) % 24, i % 60).into(),
            out: rng.below(3) == 0,
            author: NAMES[rng.below(NAMES.len())].into(),
        })
        .collect();
    let app = App::new().unwrap();
    app.set_chats(ModelRc::from(Rc::new(VecModel::from(chats))));
    app.set_msgs(ModelRc::from(Rc::new(VecModel::from(msgs))));
    println!("built data in {:?}; rss before show {:?} kB", start.elapsed(), rss_kb());
    let timer = Timer::default();
    if bench {
        let weak = app.as_weak();
        let t0 = Instant::now();
        let mut tick = 0u32;
        timer.start(TimerMode::Repeated, Duration::from_millis(16), move || {
            tick += 1;
            let Some(app) = weak.upgrade() else { return };
            // Прокручиваем обе области вниз и вверх, чтобы пройти по всем строкам.
            let phase = (tick % 400) as f32 / 400.0;
            let tri = if phase < 0.5 { phase * 2.0 } else { 2.0 - phase * 2.0 };
            let msg_h = (app.get_msg_viewport_h() - app.get_msg_visible_h()).max(0.0);
            app.set_msg_y(-(msg_h * tri));
            app.set_chat_y(-(64.0 * n_chats as f32 * tri * 0.98));
            if tick == 1 || tick == 100 || tick == 400 || tick == 800 {
                println!("tick {tick}: rss/hwm {:?} kB, {:?} elapsed", rss_kb(), t0.elapsed());
            }
            if tick >= 800 {
                println!("done: {:?} for {tick} frames, final rss/hwm {:?} kB", t0.elapsed(), rss_kb());
                slint::quit_event_loop().ok();
            }
        });
    }
    app.run().unwrap();
}
