use email_ingestor::{CHUNK, error::Kind, imap::Imap, wire::Wire};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn peek_validates_uid_offset_and_completion() {
    for suffix_uid in [false, true] {
        let (client, server) = tokio::io::duplex(1024);
        let peer = tokio::spawn(async move {
            let mut server = BufReader::new(server);
            let mut command = String::new();
            server.read_line(&mut command).await.unwrap();
            assert_eq!(
                command,
                format!("I1 UID FETCH 7 (UID BODY.PEEK[]<11.{CHUNK}>)\r\n")
            );
            let reply = if suffix_uid {
                "* 1 FETCH (BODY[]<11> {3}\r\nabc UID 7)\r\nI1 OK complete\r\n"
            } else {
                "* 1 FETCH (UID 7 BODY[]<11> {3}\r\nabc)\r\nI1 OK complete\r\n"
            };
            server.get_mut().write_all(reply.as_bytes()).await.unwrap();
        });
        let mut imap = Imap::from_wire(Wire::new(client));
        assert_eq!(imap.chunk(7, 11).await.unwrap(), b"abc");
        peer.await.unwrap();
    }
    for reply in [
        "* 1 FETCH (UID 8 BODY[]<11> {3}\r\nabc)\r\nI1 OK done\r\n",
        "* 1 FETCH (UID 7 BODY[]<12> {3}\r\nabc)\r\nI1 OK done\r\n",
        "* 1 FETCH (UID 7 BODY[]<11> {1048577}\r\n",
        "* 1 FETCH (UID 7 BODY[]<11> {3}\r\nabc)\r\nI1 NO rejected\r\n",
    ] {
        let (client, mut server) = tokio::io::duplex(1024);
        let peer = tokio::spawn(async move {
            server.write_all(reply.as_bytes()).await.unwrap();
        });
        let mut imap = Imap::from_wire(Wire::new(client));
        assert_eq!(imap.chunk(7, 11).await.unwrap_err().kind, Kind::Fatal);
        peer.await.unwrap();
    }
}
#[tokio::test]
async fn bounded_discovery_and_exact_native_move() {
    let (client, server) = tokio::io::duplex(1024);
    let peer = tokio::spawn(async move {
        let mut server = BufReader::new(server);
        let mut line = String::new();
        server.read_line(&mut line).await.unwrap();
        assert_eq!(line, "I1 UID SEARCH UID 1:100\r\n");
        server
            .get_mut()
            .write_all(b"* SEARCH 99 2 7\r\nI1 OK done\r\n")
            .await
            .unwrap();
        line.clear();
        server.read_line(&mut line).await.unwrap();
        assert_eq!(line, "I2 UID MOVE 7 \"Actual \\\"Trash\\\"\"\r\n");
        server
            .get_mut()
            .write_all(b"* 1 EXPUNGE\r\nI2 OK moved\r\n")
            .await
            .unwrap();
    });
    let mut imap = Imap::from_wire(Wire::new(client));
    assert!(imap.search(1, 101).await.is_err());
    assert_eq!(imap.search(1, 100).await.unwrap(), [2, 7, 99]);
    imap.move_uid(7, "Actual \"Trash\"").await.unwrap();
    peer.await.unwrap();
}
#[tokio::test(start_paused = true)]
async fn idle_done_is_completed_before_next_command() {
    let (client, server) = tokio::io::duplex(1024);
    let peer = tokio::spawn(async move {
        let mut server = BufReader::new(server);
        let mut line = String::new();
        server.read_line(&mut line).await.unwrap();
        assert_eq!(line, "I1 IDLE\r\n");
        server
            .get_mut()
            .write_all(b"+ idling\r\n* 1 EXISTS\r\n")
            .await
            .unwrap();
        line.clear();
        server.read_line(&mut line).await.unwrap();
        assert_eq!(line, "DONE\r\n");
        server
            .get_mut()
            .write_all(b"I1 OK idle complete\r\n")
            .await
            .unwrap();
        line.clear();
        server.read_line(&mut line).await.unwrap();
        assert_eq!(line, "I2 UID SEARCH UID 1:1\r\n");
        server
            .get_mut()
            .write_all(b"* SEARCH 1\r\nI2 OK done\r\n")
            .await
            .unwrap();
    });
    let mut imap = Imap::from_wire(Wire::new(client));
    imap.idle = true;
    imap.wait(std::time::Duration::from_secs(300))
        .await
        .unwrap();
    assert_eq!(imap.search(1, 1).await.unwrap(), [1]);
    peer.await.unwrap();
}
#[tokio::test]
async fn literal_truncation_never_returns_success() {
    let (client, mut server) = tokio::io::duplex(1024);
    let peer = tokio::spawn(async move {
        server
            .write_all(b"* 1 FETCH (UID 1 BODY[]<0> {10}\r\nshort")
            .await
            .unwrap();
    });
    let mut imap = Imap::from_wire(Wire::new(client));
    assert_eq!(imap.chunk(1, 0).await.unwrap_err().kind, Kind::Temporary);
    peer.await.unwrap();
}
#[tokio::test]
async fn wire_rejects_unbounded_peer_lines() {
    let (client, mut server) = tokio::io::duplex(1024);
    let peer = tokio::spawn(async move {
        let _ = server.write_all(&vec![b'x'; 65537]).await;
    });
    let mut wire = Wire::new(client);
    assert_eq!(wire.line(60).await.unwrap_err().kind, Kind::Fatal);
    drop(wire);
    peer.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn cancelled_line_read_preserves_partial_notification() {
    let (client, mut server) = tokio::io::duplex(1024);
    let mut wire = Wire::new(client);
    server.write_all(b"* 1 EX").await.unwrap();
    tokio::select! {
        result = wire.line(60) => panic!("unexpected completion: {result:?}"),
        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
    }
    server.write_all(b"ISTS\r\n").await.unwrap();
    assert_eq!(wire.line(60).await.unwrap(), "* 1 EXISTS");
}
