use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc::Sender;

pub struct PtyHandle {
    pub master: Box<dyn MasterPty + Send>,
    pub writer: Box<dyn Write + Send>,
    // Never read directly, but must stay alive for the lifetime of PtyHandle —
    // dropping it would kill the shell process. Silence the dead_code lint
    // rather than pretend this field is unused.
    #[allow(dead_code)]
    pub child: Box<dyn Child + Send + Sync>,
}

impl PtyHandle {
    pub fn spawn(shell: &str, cols: u16, rows: u16, on_output: Sender<Vec<u8>>) -> PtyHandle {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("failed to open pty");

        let mut cmd = CommandBuilder::new(shell);
        cmd.env("TERM", "xterm-256color");

        let child = pair.slave.spawn_command(cmd).expect("failed to spawn shell");
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().expect("failed to clone pty reader");
        let writer = pair.master.take_writer().expect("failed to get pty writer");

        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if on_output.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        PtyHandle {
            master: pair.master,
            writer,
            child,
        }
    }

    pub fn write(&mut self, data: &[u8]) {
        let _ = self.writer.write_all(data);
        let _ = self.writer.flush();
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }
}
