use anyhow::{anyhow, Result};
use argh::FromArgs;
use libnotify::Urgency;
use serde::Deserialize;
use simple_logger::SimpleLogger;
use std::fmt;
use std::time::Duration;
use tokio::select;
use tokio::signal::unix::SignalKind;

/// Get notified by libnotify whenever DClone status changes
#[derive(Debug, FromArgs)]
struct Opts {
    /// query interval (seconds)
    #[argh(option, default = "90")]
    interval: u64,

    /// ladder realm (by default, non-ladder is queried)
    #[argh(switch)]
    ladder: bool,

    /// hardcore realm (by default, softcore is queried)
    #[argh(switch)]
    hardcore: bool,

    /// don't monitor, just query the state once
    #[argh(switch)]
    oneshot: bool,
}

#[derive(Debug, Deserialize)]
struct Progress {
    progress: String,
    region: String,
}

impl From<&Progress> for i32 {
    fn from(other: &Progress) -> Self {
        str::parse(&other.progress).unwrap()
    }
}

impl fmt::Display for Progress {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let region = match self.region.as_str() {
            "1" => "Americas",
            "2" => "Europe",
            "3" => "Asia",
            _ => "Unknown",
        };

        write!(f, "Progress for {}: {}/6", region, self.progress)
    }
}

#[derive(Debug, Default, PartialEq, Copy, Clone)]
struct Status {
    americas: i32,
    europe: i32,
    asia: i32,
}

impl Status {
    fn update_americas(&mut self, new: i32) -> Result<()> {
        if new != self.americas {
            notify("Americas", self.americas, new)?;
            self.americas = new;
        }

        Ok(())
    }

    fn update_europe(&mut self, new: i32) -> Result<()> {
        if new != self.europe {
            notify("Europe", self.europe, new)?;
            self.europe = new;
        }

        Ok(())
    }

    fn update_asia(&mut self, new: i32) -> Result<()> {
        if new != self.asia {
            notify("Asia", self.asia, new)?;
            self.asia = new;
        }

        Ok(())
    }
}

fn notify(region: &str, old: i32, new: i32) -> Result<()> {
    let (title, urgency) = match new {
        1 => ("DClone is far away", Urgency::Low),
        2 | 3 | 4 => ("DClone is nearing...", Urgency::Normal),
        5 => ("DClone is about to walk!", Urgency::Critical),
        6 => ("DClone is walking!", Urgency::Critical),
        n => return Err(anyhow!("Unknown progress value: {}", n)),
    };

    let msg = if old == 0 {
        format!("New status: {}", new)
    } else {
        format!("Status changed from {} to {}", old, new)
    };

    let title = format!("{}: {}", region, title);

    let notification = libnotify::Notification::new(&title, Some(msg.as_str()), Some("annihilus"));
    notification.set_urgency(urgency);
    notification.show()?;
    Ok(())
}

fn build_client() -> Result<reqwest::Client> {
    let client = reqwest::Client::builder()
        .user_agent("dclone-tracker/0.1.0 https://github.com/tronje/dclone-tracker")
        .build()?;
    Ok(client)
}

fn build_url(ladder: bool, hardcore: bool) -> String {
    let ladder = if ladder { 1 } else { 2 };
    let hardcore = if hardcore { 1 } else { 2 };
    format!(
        "https://diablo2.io/dclone_api.php?ladder={}&hc={}",
        ladder, hardcore
    )
}

async fn run_once(opts: Opts) -> Result<()> {
    let url = build_url(opts.ladder, opts.hardcore);
    let client = build_client()?;

    let response = client
        .get(&url)
        .send()
        .await?
        .json::<Vec<Progress>>()
        .await?;
    for progress in response {
        log::info!("{}", progress);
    }
    Ok(())
}

async fn run(opts: Opts) -> Result<()> {
    let url = build_url(opts.ladder, opts.hardcore);

    let mut timer = tokio::time::interval(Duration::from_secs(opts.interval));
    let client = build_client()?;

    let mut sigint = tokio::signal::unix::signal(SignalKind::interrupt())?;
    let mut sigterm = tokio::signal::unix::signal(SignalKind::terminate())?;

    let mut status = Status::default();

    loop {
        select! {
            _ = sigint.recv() => {
                log::info!("Interrupted.");
                break;
            }

            _ = sigterm.recv() => {
                log::info!("Terminated.");
                break;
            }

            _ = timer.tick() => {
                let response = match client.get(&url).send().await?.json::<Vec<Progress>>().await {
                    Ok(values) => values,
                    Err(e) => {
                        log::error!("{}", e);
                        continue;
                    }
                };

                log::debug!("Received response: {:#?}", response);

                for progress in response {
                    match progress.region.as_str() {
                        "1" => status.update_americas(str::parse(&progress.progress)?)?,
                        "2" => status.update_europe(str::parse(&progress.progress)?)?,
                        "3" => status.update_asia(str::parse(&progress.progress)?)?,
                        n => log::warn!("Unexpected region code: {}", n),
                    }
                }
            }
        }
    }

    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let opts = argh::from_env::<Opts>();

    SimpleLogger::new()
        .with_level(log::LevelFilter::Debug)
        .init()?;

    log::info!("Data courtesy of diablo2.io");

    if opts.oneshot {
        run_once(opts).await?;
        return Ok(());
    }

    libnotify::init("dclone-tracker").map_err(|e| anyhow!("{}", e))?;
    let result = run(opts).await;
    libnotify::uninit();

    result
}
