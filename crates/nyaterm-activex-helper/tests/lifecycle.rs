use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use nyaterm_remote_desktop::activex::{HostCommand, HostEvent, decode, encode};
use nyaterm_remote_desktop::{PROTOCOL_VERSION, read_packet, write_packet};

fn pump_parent_messages() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage,
        };
        let mut message = MSG::default();
        for _ in 0..64 {
            if !PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                break;
            }
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

struct Host {
    child: Child,
    input: Option<ChildStdin>,
    events: Receiver<HostEvent>,
}

impl Host {
    fn new() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_nyaterm-activex-helper"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let mut output = child.stdout.take().unwrap();
        let (tx, events) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(Some(packet)) = read_packet(&mut output) {
                let Ok(event) = decode(&packet) else { break };
                if tx.send(event).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            input,
            events,
        }
    }

    fn send(&mut self, command: HostCommand) {
        write_packet(self.input.as_mut().unwrap(), &encode(&command).unwrap()).unwrap();
    }

    fn event(&self) -> HostEvent {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            pump_parent_messages();
            if let Ok(event) = self.events.try_recv() {
                return event;
            }
            assert!(Instant::now() < deadline, "host response timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn hello(&mut self) {
        self.send(HostCommand::Hello {
            version: PROTOCOL_VERSION,
            parent_pid: std::process::id(),
        });
        assert!(matches!(
            self.event(),
            HostEvent::Hello {
                version: PROTOCOL_VERSION
            }
        ));
    }

    fn exited(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            pump_parent_messages();
            if self.child.try_wait().unwrap().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("host did not exit");
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn handshake_heartbeat_and_ordinary_disconnect() {
    let mut host = Host::new();
    host.hello();
    host.send(HostCommand::Ping);
    assert!(matches!(host.event(), HostEvent::Pong));
    host.send(HostCommand::Close);
    assert!(matches!(host.event(), HostEvent::Closed));
    host.exited();
}

#[test]
fn incompatible_protocol_is_rejected_before_a_session_starts() {
    let mut host = Host::new();
    host.send(HostCommand::Hello {
        version: PROTOCOL_VERSION - 1,
        parent_pid: std::process::id(),
    });
    assert!(matches!(host.event(), HostEvent::Connection(_)));
    host.exited();
}

#[test]
fn parent_pipe_eof_reaps_an_idle_host() {
    let mut host = Host::new();
    host.hello();
    host.input.take();
    host.exited();
}

#[test]
fn a_crashed_host_closes_its_event_pipe() {
    let mut host = Host::new();
    host.hello();
    host.child.kill().unwrap();
    host.exited();
    assert!(host.events.recv_timeout(Duration::from_secs(2)).is_err());
}

#[cfg(windows)]
struct Parent(windows::Win32::Foundation::HWND);
#[cfg(windows)]
impl Drop for Parent {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.0);
        }
    }
}

#[cfg(windows)]
fn prepared_host(
    server: &str,
    port: u16,
) -> (
    Parent,
    Host,
    windows::Win32::Foundation::HWND,
    nyaterm_remote_desktop::activex::NativeRdpRect,
) {
    use nyaterm_remote_desktop::activex::NativeRdpRect;
    use std::ffi::c_void;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, GetParent, IsWindowVisible, WINDOW_EX_STYLE, WS_OVERLAPPED,
    };
    use windows::core::w;

    // The parent belongs to the test process; the COM control belongs to the
    // child helper. No server connection or saved credentials are involved.
    let parent = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!(""),
            WS_OVERLAPPED,
            0,
            0,
            800,
            600,
            None,
            None,
            None,
            None,
        )
    }
    .unwrap();
    let mut host = Host::new();
    host.hello();
    let config = serde_json::from_value(serde_json::json!({
        "backend":"windows", "windows_authentication":"system", "name":"probe", "host":server, "port":port,
        "username":"", "domain":"", "password":null, "use_nla":true, "certificate_policy":"strict",
        "display":{"mode":"fit_window","width":800,"height":600,"color_depth":32,"desktop_scale_factor":150},
        "clipboard":{"mode":"disabled"}, "reconnect":{"enabled":false,"max_attempts":0}
    })).unwrap();
    let rect = NativeRdpRect {
        x: 0,
        y: 0,
        width: 800,
        height: 600,
        scale_percent: 150,
    };
    host.send(HostCommand::Prepare {
        parent: parent.0 as usize as u64,
        rect,
        config: Box::new(config),
    });
    let event = host.event();
    let HostEvent::Prepared { child } = event else {
        panic!("cannot prepare host: {event:?}")
    };
    let child = HWND(child as usize as *mut c_void);
    assert_eq!(unsafe { GetParent(child) }.unwrap(), parent);
    assert!(!unsafe { IsWindowVisible(child).as_bool() });
    (Parent(parent), host, child, rect)
}

#[cfg(windows)]
#[test]
fn native_child_can_be_embedded_hidden_and_shown_without_connecting() {
    let (_parent, mut host, child, rect) = prepared_host("example.invalid", 3389);
    for visible in [true, false, true, false] {
        host.send(HostCommand::Viewport { rect, visible });
        host.send(HostCommand::Ping);
        assert!(matches!(host.event(), HostEvent::Pong));
        use windows::Win32::UI::WindowsAndMessaging::{GWL_STYLE, GetWindowLongW, WS_VISIBLE};
        assert_eq!(
            unsafe { GetWindowLongW(child, GWL_STYLE) } as u32 & WS_VISIBLE.0 != 0,
            visible
        );
    }
    host.send(HostCommand::Close);
    assert!(matches!(host.event(), HostEvent::Closed));
    host.exited();
}

#[cfg(windows)]
#[test]
fn first_connection_failure_is_reported_without_a_previous_connected_event() {
    use nyaterm_remote_desktop::activex::NativeRdpConnectionEvent;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let (_parent, mut host, _, _) = prepared_host("127.0.0.1", port);
    host.send(HostCommand::Start);
    let deadline = Instant::now() + Duration::from_secs(35);
    loop {
        pump_parent_messages();
        if let Ok(HostEvent::Connection(event)) = host.events.try_recv() {
            match event {
                NativeRdpConnectionEvent::Failed(_) | NativeRdpConnectionEvent::Disconnected(_) => {
                    break;
                }
                NativeRdpConnectionEvent::Connected => panic!("closed port connected"),
                NativeRdpConnectionEvent::Reconnecting => {}
            }
        }
        assert!(Instant::now() < deadline, "first attempt stayed connecting");
        std::thread::sleep(Duration::from_millis(5));
    }
    host.send(HostCommand::Close);
    assert!(matches!(host.event(), HostEvent::Closed));
    host.exited();
}

#[cfg(windows)]
#[test]
fn parent_pipe_eof_reaps_a_host_even_when_its_sta_is_stuck() {
    use windows::Win32::System::Threading::{OpenThread, SuspendThread, THREAD_SUSPEND_RESUME};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let (_parent, mut host, child, _) = prepared_host("example.invalid", 3389);
    let thread_id = unsafe { GetWindowThreadProcessId(child, None) };
    let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, false, thread_id) }.unwrap();
    assert_ne!(unsafe { SuspendThread(thread) }, u32::MAX);
    unsafe {
        windows::Win32::Foundation::CloseHandle(thread).unwrap();
    }
    host.input.take();
    // The independent parent watcher must release the process even though the
    // STA cannot process EOF, Close, a heartbeat or a COM destructor.
    host.exited();
}
