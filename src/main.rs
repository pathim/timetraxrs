#![windows_subsystem = "windows"]
use chrono::Duration;
use iced::executor;
use iced::widget::{button, container, radio, text, text_input, Column, Row};
use iced::{Application, Command, Element, Length, Settings, Subscription, Theme};

mod database;

use database::Database;

pub fn main() -> iced::Result {
    let settings = Settings {
        window: iced::window::Settings {
            size: (300, 500),
            resizable: false,
            decorations: true,
            ..Default::default()
        },
        ..Default::default()
    };
    Timetrax::run(settings)
}

struct Timetrax {
    now: chrono::DateTime<chrono::Local>,
    db: Database,
    current_work: Option<u64>,
    available_work: Vec<(String, u64)>,
    work_times: std::collections::HashMap<u64, Duration>,
    new_work_item: String,
    net_time: Duration,
}

#[derive(Debug, Clone)]
enum Message {
    Tick(chrono::DateTime<chrono::Local>),
    ChangeWork(Option<u64>),
    TypeNewItem(String),
    AddNewWork,
}

fn format_duration(duration: &Duration) -> String {
    let sign = if *duration < Duration::zero() {
        "-"
    } else {
        ""
    };
    let hours = duration.num_hours().abs();
    let minutes = duration.num_minutes().abs() - 60 * hours;
    let seconds = duration.num_seconds().abs() - 60 * (minutes + 60 * hours);
    format!("{}{:02}:{:02}:{:02}", sign, hours, minutes, seconds)
}

impl Application for Timetrax {
    type Executor = executor::Default;
    type Message = Message;
    type Theme = Theme;
    type Flags = ();

    fn new(_flags: ()) -> (Self, Command<Message>) {
        let db = Database::open("work.db", chrono::Utc::now).unwrap();
        let net_time = db.get_time_diff().unwrap() - db.get_expected_today().unwrap();
        let available_work = db.get_available_work().unwrap();
        let current_work = db.get_current_work().unwrap();
        (
            Timetrax {
                now: chrono::Local::now(),
                db,
                current_work,
                available_work,
                work_times: Default::default(),
                new_work_item: Default::default(),
                net_time,
            },
            Command::none(),
        )
    }

    fn title(&self) -> String {
        String::from("Timetrax")
    }

    fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Tick(local_time) => {
                let now = local_time;

                if now != self.now {
                    self.now = now;
                    if let Ok(mut work_time) = self.db.get_work_today() {
                        work_time.push((None, now));
                        let mut times = std::collections::HashMap::new();
                        for work in work_time.windows(2) {
                            let start = &work[0];
                            let end = &work[1];
                            if let Some(work_item) = start.0 {
                                let duration = end.1 - start.1;
                                let worked = times.entry(work_item).or_insert_with(Duration::zero);
                                *worked = *worked + duration;
                            }
                        }
                        self.work_times = times;
                    }
                }
            }
            Message::ChangeWork(v) => {
                if self.current_work != v {
                    let err = self.db.set_current_work(v);
                    match err {
                        Ok(_) => {
                            self.current_work = v;
                        }
                        Err(e) => {
                            println!("{:?}", e);
                        }
                    }
                }
            }
            Message::TypeNewItem(s) => {
                self.new_work_item = s;
            }
            Message::AddNewWork => {
                if self.db.add_work_item(&self.new_work_item).is_ok() {
                    self.new_work_item.clear();
                }
                if let Ok(work) = self.db.get_available_work() {
                    self.available_work = work;
                }
            }
        }

        Command::none()
    }

    fn view(&self) -> Element<Message> {
        let col1_width = Length::Units(150);
        let mut col = Column::new();
        let pause_button = radio("Pause", None, Some(self.current_work), Message::ChangeWork)
            .width(Length::Units(150));
        col = col.push(pause_button);
        let mut total_time = Duration::zero();
        for (name, id) in &self.available_work {
            let mut row = Row::new();
            let button = radio(
                name,
                Some(*id),
                Some(self.current_work),
                Message::ChangeWork,
            )
            .width(col1_width);
            row = row.push(button);
            if let Some(duration) = self.work_times.get(id) {
                total_time = total_time + *duration;
                row = row.push(text(format_duration(duration)));
            }
            col = col.push(row);
        }
        col = col.push(
            Row::new()
                .push(
                    text_input("new work item", &self.new_work_item, Message::TypeNewItem)
                        .width(col1_width)
                        .on_submit(Message::AddNewWork),
                )
                .push(button(text("+")).on_press(Message::AddNewWork)),
        );

        col = col.push(
            Row::new()
                .push(text("Total time today").width(col1_width))
                .push(text(format_duration(&total_time))),
        );
        col = col.push(
            Row::new()
                .push(text("Total net time").width(col1_width))
                .push(text(format_duration(&(self.net_time + total_time)))),
        );
        container(col)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x()
            .into()
    }

    fn subscription(&self) -> Subscription<Message> {
        iced::time::every(std::time::Duration::from_millis(900))
            .map(|_| Message::Tick(chrono::Local::now()))
    }
}
