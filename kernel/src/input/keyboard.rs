//! Architecture-neutral keyboard event queue.

use spin::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyInput {
    Char(u8),
    Tab { shift: bool },
    Left,
    Right,
    Up,
    Down,
    Backspace,
    Enter,
}

#[derive(Clone, Copy)]
struct KeyboardQueue {
    buffer: [Option<KeyInput>; 64],
    head: usize,
    length: usize,
}

impl KeyboardQueue {
    const fn new() -> Self {
        Self {
            buffer: [None; 64],
            head: 0,
            length: 0,
        }
    }

    fn push(&mut self, input: KeyInput) {
        let tail = (self.head + self.length) % self.buffer.len();
        self.buffer[tail] = Some(input);
        if self.length == self.buffer.len() {
            self.head = (self.head + 1) % self.buffer.len();
        } else {
            self.length += 1;
        }
    }

    fn pop(&mut self) -> Option<KeyInput> {
        if self.length == 0 {
            return None;
        }
        let event = self.buffer[self.head].take();
        self.head = (self.head + 1) % self.buffer.len();
        self.length -= 1;
        event
    }
}

static QUEUE: Mutex<KeyboardQueue> = Mutex::new(KeyboardQueue::new());

pub fn push(input: KeyInput) {
    crate::arch::interrupts::without_interrupts(|| QUEUE.lock().push(input));
    crate::input::diagnostics::record_key(input);
    crate::sched::notify_interactive_input();
}

pub fn try_read_key() -> Option<KeyInput> {
    crate::arch::interrupts::without_interrupts(|| QUEUE.lock().pop())
}

pub fn has_pending_key() -> bool {
    crate::arch::interrupts::without_interrupts(|| QUEUE.lock().length != 0)
}
