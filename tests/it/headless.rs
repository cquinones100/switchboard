//! `switchboard headless`, `headless-keys` and `headless-screen` on a test
//! server: a client with no terminal reads the whole screen (mission
//! control over every pane), as text and with its colors, and types keys by
//! name;
//! the server named by `$SWITCHBOARD`, the workspace you are in, is refused
//! without `--yes`.

use crate::common;

use common::TestServer;

#[test]
fn a_headless_client_reads_the_whole_screen_and_types_keys() {
    let server = TestServer::start();
    let (code, out, err) = server.run_binary(&["headless", "--size", "120x32"]);
    assert_eq!(code, 0, "{err}");
    let id = out.trim().to_string();

    let (code, _, err) = server.run_binary(&["headless-keys", &id, "M-Space"]);
    assert_eq!(code, 0, "{err}");
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    let screen = loop {
        let (_, screen, _) = server.run_binary(&["headless-screen", &id]);
        if screen.contains("MISSION CONTROL") || std::time::Instant::now() > deadline {
            break screen;
        }
    };
    assert!(screen.contains("MISSION CONTROL"), "{screen}");
    assert!(screen.contains("1 home"), "its tab's card: {screen}");
    assert_eq!(screen.lines().count(), 32, "{screen}");

    let (_, ansi, _) = server.run_binary(&["headless-screen", &id, "--ansi"]);
    assert!(ansi.contains("\x1b[0;"), "{ansi:?}");
    assert!(
        ansi.contains("38;2;"),
        "colors as red, green and blue: {ansi:?}"
    );

    if switchboard::views::image::render::any_chrome().is_some() {
        let png = server.dir.path().join("screen.png");
        let (code, _, err) =
            server.run_binary(&["headless-screen", &id, "--png", &png.to_string_lossy()]);
        // A Chrome that can't run here (a CI runner's, refused its sandbox)
        // is said plainly; one that runs draws the screen at its size.
        if code == 0 {
            let size = image::image_dimensions(&png).unwrap();
            let cell = switchboard::ui::headless::CELL;
            assert!(size.0 >= 120 * cell.0 && size.1 >= 32 * cell.1, "{size:?}");
        } else {
            assert!(err.contains("Chrome didn't draw"), "{err}");
        }
    }

    let (code, _, err) = server.run_binary(&["headless", "--stop", &id]);
    assert_eq!(code, 0, "{err}");
    let (code, _, _) = server.run_binary(&["headless-screen", &id]);
    assert_ne!(code, 0, "the client is gone");
}

#[test]
fn the_workspace_you_are_in_is_refused_without_yes() {
    let server = TestServer::start();
    let out = std::process::Command::new(common::BIN)
        .args(["headless"])
        .env("HOME", &server.home)
        .env("SWITCHBOARD", server.socket())
        .env("SWITCHBOARD_STATE_DIR", &server.state)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("the workspace you're in"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_headless_client_ends_with_its_server() {
    let server = TestServer::start();
    let (code, out, err) = server.run_binary(&["headless"]);
    assert_eq!(code, 0, "{err}");
    let socket = server.state.join(format!("headless-{}.sock", out.trim()));
    assert!(socket.exists());
    server.ok(&["kill-server"]);
    let deadline = common::deadline(std::time::Duration::from_secs(10));
    while socket.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "the client outlived its server"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[test]
fn a_headless_client_attaches_as_ghostty_and_keeps_the_servers_images() {
    use switchboard::ui::headless::{self, Client};
    let server = TestServer::start();
    let truecolor_cells = |client: &Client| {
        client.read(|s| {
            let (cols, rows) = s.size();
            (0..rows)
                .flat_map(|y| (0..cols).map(move |x| (y, x)))
                .filter(|&(y, x)| s.fg_rgb(y, x).is_some())
                .count()
        })
    };
    // Mission control, then the palette's box over it, whose shadow is
    // an image.
    let open_palette = |term: &str| {
        let mut client = Client::attach(&server.socket(), 120, 32, None, term).unwrap();
        client.keys(&["M-Space".to_string()]).unwrap();
        assert!(
            client.wait_text(
                "MISSION CONTROL",
                std::time::Duration::from_secs(10) * common::patience()
            ),
            "{}",
            client.read(|s| s.text().join("\n"))
        );
        client
            .keys(&["Escape".to_string(), "M-p".to_string()])
            .unwrap();
        let deadline = common::deadline(std::time::Duration::from_secs(5));
        while std::time::Instant::now() < deadline
            && !client.read(|s| s.html(9, 18).contains("<img"))
        {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        client
    };

    let mut ghostty = open_palette(headless::TERM);
    let html = ghostty.read(|s| s.html(headless::CELL.0, headless::CELL.1));
    assert!(
        html.contains("<img class=u src=\"data:image/png;base64,"),
        "the box's shadow, an image: {html}"
    );
    let ghostty_colors = truecolor_cells(&ghostty);
    ghostty.detach();

    let mut plain = open_palette("xterm-256color");
    let html = plain.read(|s| s.html(headless::CELL.0, headless::CELL.1));
    assert!(!html.contains("base64"), "no images: {html}");
    assert!(
        ghostty_colors > truecolor_cells(&plain) * 2,
        "the canvas's colors for Ghostty: {ghostty_colors} against {}",
        truecolor_cells(&plain)
    );
    plain.detach();
}

#[test]
fn a_headless_client_with_no_size_is_wide_enough_that_no_view_stacks() {
    let server = TestServer::start();
    let (cols, rows) = switchboard::ui::layout::SIDE_BY_SIDE;
    let (code, out, err) = server.run_binary(&["headless", "--min-size"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), format!("{cols}x{rows}"));

    let (code, out, err) = server.run_binary(&["headless"]);
    assert_eq!(code, 0, "{err}");
    let id = out.trim().to_string();
    let (code, screen, err) = server.run_binary(&["headless-screen", &id]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(screen.lines().count(), rows as usize, "{screen}");
    let clients = server.list()["clients"].clone();
    assert_eq!(clients[0]["cols"], cols, "{clients}");
    server.run_binary(&["headless", "--stop", &id]);
}
