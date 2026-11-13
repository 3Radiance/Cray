#![cfg(target_os = "linux")]

use std::ffi::c_void;
use std::io;
use std::net::SocketAddr;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::sync::Arc;

use cray_layers::tun::TunProtect;
use cray_layers::tun_tcp::{SocketProtect, clear_protect, get_protect, set_protect};

fn tcp_socket() -> tokio::net::TcpSocket {
    let sock = tokio::net::TcpSocket::new_v4().expect("new socket");
    let addr: SocketAddr = "127.0.0.1:0".parse().expect("addr");
    sock.bind(addr).expect("bind");
    sock
}

/// SO_MARK needs CAP_NET_ADMIN (or CAP_NET_RAW since Linux 5.17). Probe with a
/// throwaway socket so unprivileged runs skip instead of failing.
fn can_set_mark() -> Result<(), String> {
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").expect("udp bind");
    let mark: u32 = 1;
    let ret = unsafe {
        libc::setsockopt(
            sock.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_MARK,
            &mark as *const u32 as *const c_void,
            std::mem::size_of::<u32>() as libc::socklen_t,
        )
    };
    if ret == 0 {
        return Ok(());
    }
    match io::Error::last_os_error().raw_os_error() {
        Some(libc::EPERM) => Err("no CAP_NET_ADMIN/CAP_NET_RAW for SO_MARK".into()),
        Some(libc::ENOPROTOOPT) => Err("SO_MARK not supported by this kernel".into()),
        Some(code) => Err(format!("unexpected setsockopt error: os code {code}")),
        None => Err("setsockopt failed with no errno".into()),
    }
}

fn get_mark(fd: BorrowedFd<'_>) -> io::Result<u32> {
    let mut mark: u32 = 0;
    let mut len = std::mem::size_of::<u32>() as libc::socklen_t;
    let ret = unsafe {
        libc::getsockopt(
            fd.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_MARK,
            &mut mark as *mut u32 as *mut c_void,
            &mut len,
        )
    };
    if ret == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(mark)
}

#[test]
fn protect_object_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Arc<dyn SocketProtect>>();
    assert_send_sync::<TunProtect>();
}

#[test]
fn set_mark_fails_on_non_socket_fd() {
    // ENOTSOCK is raised before any capability check, so this test works
    // unprivileged.
    let file = std::fs::File::open("/dev/null").expect("open /dev/null");
    let protect = TunProtect::new(0x42);
    let res = protect.set_mark(file.as_fd());
    let err = res.expect_err("setsockopt(SO_MARK) must fail on a non-socket fd");
    assert!(
        err.to_string().contains("Failed to set mark"),
        "unexpected error: {err:#}"
    );
}

#[tokio::test]
async fn so_mark_roundtrip_on_tcp_socket() {
    if let Err(reason) = can_set_mark() {
        eprintln!("skip: {reason}");
        return;
    }

    let sock = tcp_socket();
    let protect = TunProtect::new(0x2a);
    protect.protect(sock.as_fd()).expect("set mark");

    assert_eq!(get_mark(sock.as_fd()).expect("get mark"), 0x2a);
}

#[tokio::test]
async fn so_mark_can_be_updated_in_place() {
    if let Err(reason) = can_set_mark() {
        eprintln!("skip: {reason}");
        return;
    }

    let sock = tcp_socket();
    let protect = TunProtect::new(0x1);
    protect.protect(sock.as_fd()).expect("set first mark");
    assert_eq!(get_mark(sock.as_fd()).expect("get mark"), 0x1);

    let protect = TunProtect::new(0x2);
    protect.protect(sock.as_fd()).expect("set second mark");
    assert_eq!(get_mark(sock.as_fd()).expect("get mark"), 0x2);
}

#[tokio::test]
async fn protect_registry_applies_mark() {
    if let Err(reason) = can_set_mark() {
        eprintln!("skip: {reason}");
        return;
    }

    clear_protect();
    assert!(get_protect().is_none(), "registry must start empty");

    set_protect(TunProtect::new(0x1234));
    let protect = get_protect().expect("protect registered");

    let sock = tcp_socket();
    protect.protect(sock.as_fd()).expect("protect socket");
    assert_eq!(get_mark(sock.as_fd()).expect("get mark"), 0x1234);

    clear_protect();
    assert!(
        get_protect().is_none(),
        "registry must be empty after clear"
    );
}
