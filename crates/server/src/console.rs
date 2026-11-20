//! Local operator input. A closed stdin leaves a daemon running.
use std::io::{self, BufRead};
use std::sync::mpsc::{self, Receiver};

pub fn read_commands() -> Receiver<String> {
    let (sender, receiver) = mpsc::sync_channel(16);
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            if line.len() <= 256 && sender.send(line).is_err() {
                break;
            }
        }
    });
    receiver
}
