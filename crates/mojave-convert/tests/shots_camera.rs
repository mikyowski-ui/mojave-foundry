//! Stationary versus following replay logs, and the re-centre decision.
//! No engine is run: these check the records written, which the engine
//! was seen to read as described in `mojave_convert::replay`.

use mojave_convert::replay::{
    CameraPlan, FINAL_TICKS, Hop, LogOptions, Recentre, SETTLE_TICKS, View, build_log, fit_steps,
    frame_size, layout, plan_camera,
};
use mojave_convert::route::Route;
use mojave_convert::shots::{CameraKind, Framing, framed_hop, hop_ticks, replay_log, route_hops};
use vault_formats::HexGrid;

fn hex(c: i32, r: i32) -> i32 {
    c + 200 * r
}

fn centre(hex: i32) -> [f64; 2] {
    let [x, y] = HexGrid::default().hex_center(hex).unwrap();
    [x as f64, y as f64]
}

fn records(log: &str) -> Vec<serde_json::Value> {
    log.lines()
        .map(|l| serde_json::from_str(l).expect("a JSON line"))
        .collect()
}

/// A straight walk along row 100 from column `from`, `hop` steps a hop.
fn row_walk(from: i32, to: i32, hop: usize) -> (i32, Vec<Hop>) {
    let path: Vec<i32> = if to >= from {
        (from..=to).map(|c| hex(c, 100)).collect()
    } else {
        (to..=from).rev().map(|c| hex(c, 100)).collect()
    };
    let route = Route {
        requested: *path.last().unwrap(),
        destination: *path.last().unwrap(),
        path,
    };
    (route.path[0], route_hops(&route, hop, &[], hop_ticks))
}

#[test]
fn a_following_log_has_no_pointer_or_key_records() {
    let log = replay_log(View::Classic, &[(hex(103, 100), 3)], 1.0);
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(
        lines[0],
        "{\"command_id\":1,\"kind\":\"surface\",\"width\":640,\"height\":480}"
    );
    assert_eq!(
        lines[1],
        "{\"command_id\":2,\"kind\":\"enhanced\",\"settings\":{\"viewport_mode\":\"unlocked_widescreen\",\
         \"default_zoom\":1.0,\"smooth_camera\":false,\"roof_peek\":\"hidden\",\"ui_scale\":null}}"
    );
    assert_eq!(
        lines[2 + SETTLE_TICKS as usize],
        format!(
            "{{\"command_id\":{},\"kind\":\"bot\",\"action\":{{\"op\":\"walk\",\"hex\":20103,\"run\":true}}}}",
            3 + SETTLE_TICKS
        )
    );
    assert_eq!(
        lines.len() as u64,
        2 + SETTLE_TICKS + 1 + hop_ticks(3) + FINAL_TICKS
    );
    for kind in ["pointer", "key", "Middle", "Home"] {
        assert!(!log.contains(kind), "{kind} in a following log");
    }
}

#[test]
fn a_stationary_log_freezes_the_camera_after_the_settle_ticks() {
    let start = hex(100, 100);
    let hops = vec![Hop {
        hex: hex(103, 100),
        steps: 3,
        ticks: 10,
        path: vec![start, hex(101, 100), hex(102, 100), hex(103, 100)],
    }];
    // A plan centred on the player's own hex: a zero-length drag.
    let plan = CameraPlan {
        start_hex: start,
        centre: centre(start),
        centre_hex: Some(start),
        canvas: [640, 380],
        margin: 0.15,
        recentres: Vec::new(),
    };
    let options = LogOptions {
        view: View::Classic,
        zoom: 1.0,
        run: true,
        settle: 2,
        tail: 1,
    };
    let log = build_log(&options, &hops, Some(&plan)).text();
    let expected = [
        "{\"command_id\":1,\"kind\":\"surface\",\"width\":640,\"height\":480}",
        "{\"command_id\":2,\"kind\":\"enhanced\",\"settings\":{\"viewport_mode\":\"unlocked_widescreen\",\"default_zoom\":1.0,\"smooth_camera\":false,\"roof_peek\":\"hidden\",\"ui_scale\":null}}",
        "{\"command_id\":3,\"kind\":\"tick\",\"now_ms\":16}",
        "{\"command_id\":4,\"kind\":\"tick\",\"now_ms\":32}",
        "{\"command_id\":5,\"kind\":\"pointer_moved\",\"x\":320.000,\"y\":190.000}",
        "{\"command_id\":6,\"kind\":\"pointer_button\",\"button\":\"Middle\",\"pressed\":true}",
        "{\"command_id\":7,\"kind\":\"pointer_moved\",\"x\":320.000,\"y\":190.000}",
        "{\"command_id\":8,\"kind\":\"pointer_button\",\"button\":\"Middle\",\"pressed\":false}",
        "{\"command_id\":9,\"kind\":\"pointer_left\"}",
        "{\"command_id\":10,\"kind\":\"bot\",\"action\":{\"op\":\"walk\",\"hex\":20103,\"run\":true}}",
    ];
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(&lines[..expected.len()], &expected);
    assert_eq!(lines.len(), expected.len() + 10 + 1);

    // The same walk following: the freeze is the only difference.
    let follow = build_log(&options, &hops, None).text();
    let follow: Vec<&str> = follow.lines().collect();
    assert_eq!(follow.len() + 5, lines.len());
    assert!(follow.iter().all(|l| !l.contains("pointer")));

    // The game view is never frozen, plan or not.
    let game = LogOptions {
        view: View::Game,
        ..options
    };
    let text = build_log(&game, &hops, Some(&plan)).text();
    assert!(!text.contains("pointer") && !text.contains("enhanced"));
}

#[test]
fn a_recentre_is_home_a_tick_and_a_panned_freeze() {
    let start = hex(100, 100);
    let at = hex(110, 100);
    let hops = vec![
        Hop {
            hex: at,
            steps: 10,
            ticks: 3,
            path: vec![start, at],
        },
        Hop {
            hex: hex(120, 100),
            steps: 10,
            ticks: 3,
            path: vec![at, hex(120, 100)],
        },
    ];
    let new_centre = [centre(at)[0] + 100.0, centre(at)[1] - 40.0];
    let plan = CameraPlan {
        start_hex: start,
        centre: centre(start),
        centre_hex: Some(start),
        canvas: [1138, 507],
        margin: 0.15,
        recentres: vec![Recentre {
            before_hop: 1,
            player_hex: at,
            centre: new_centre,
            centre_hex: None,
        }],
    };
    let options = LogOptions {
        view: View::Wide,
        zoom: 0.75,
        run: false,
        settle: 1,
        tail: 0,
    };
    let built = build_log(&options, &hops, Some(&plan));
    let lines = records(&built.text());
    let home = lines
        .iter()
        .position(|l| l["kind"] == "key")
        .expect("a Home key");
    // After the first hop's walk and its 3 ticks.
    let first_walk = lines.iter().position(|l| l["kind"] == "bot").unwrap();
    assert_eq!(home, first_walk + 4);
    assert_eq!(lines[home]["physical_key"], "Home");
    assert_eq!(lines[home]["pressed"], true);
    assert_eq!(lines[home]["repeat"], false);
    assert_eq!(lines[home + 1]["physical_key"], "Home");
    assert_eq!(lines[home + 1]["pressed"], false);
    assert_eq!(lines[home + 2]["kind"], "tick");
    assert_eq!(lines[home + 3]["kind"], "pointer_moved");
    assert_eq!(lines[home + 4]["button"], "Middle");
    assert_eq!(lines[home + 5]["kind"], "pointer_moved");
    assert_eq!(lines[home + 6]["pressed"], false);
    assert_eq!(lines[home + 7]["kind"], "pointer_left");
    assert_eq!(lines[home + 8]["kind"], "bot");
    assert_eq!(lines[home + 8]["action"]["run"], false);
    // The pointer moves against the pan, scaled to surface pixels.
    let per_world = layout([1920, 1080], 0.75).per_world;
    let dx = lines[home + 3]["x"].as_f64().unwrap() - lines[home + 5]["x"].as_f64().unwrap();
    let dy = lines[home + 3]["y"].as_f64().unwrap() - lines[home + 5]["y"].as_f64().unwrap();
    assert!((dx - 100.0 * per_world).abs() < 0.01, "{dx}");
    assert!((dy + 40.0 * per_world).abs() < 0.01, "{dy}");
    // Inside the world area, away from the edge band.
    for l in lines.iter().filter(|l| l["kind"] == "pointer_moved") {
        let (x, y) = (l["x"].as_f64().unwrap(), l["y"].as_f64().unwrap());
        assert!((0.05 * 1920.0..0.95 * 1920.0).contains(&x), "{x}");
        assert!((0.05 * 855.0..0.95 * 855.0).contains(&y), "{y}");
    }
    // No frame is cut before the first freeze or between Home and its
    // freeze.
    assert!(!built.settled(1));
    let home_tick = 1 + 3 + 1;
    assert!(built.settled(home_tick - 1));
    assert!(!built.settled(home_tick));
    assert!(built.settled(home_tick + 1));
    assert_eq!(built.next_settled(home_tick), Some(home_tick + 1));
    assert_eq!(built.recentre_ticks(), &[home_tick + 1]);
    let (cut, lines_cut) = built.cut(home_tick + 1);
    assert_eq!(cut.lines().count(), lines_cut);
    assert!(cut.trim_end().ends_with("\"now_ms\":96}"));
    assert!(cut.contains("pointer_left"));
}

#[test]
fn a_walk_that_fits_is_framed_once_and_never_recentred() {
    let (start, hops) = row_walk(100, 108, 4);
    let plan = plan_camera(start, &hops, [640, 380], 0.15);
    assert!(plan.recentres.is_empty(), "{:?}", plan.recentres);
    // The middle of the walk's drawn box.
    let (a, b) = (centre(hex(100, 100)), centre(hex(108, 100)));
    let x = (a[0].min(b[0]) - 16.0 + a[0].max(b[0]) + 16.0) / 2.0;
    assert_eq!(plan.centre[0], x);
    assert_eq!(
        plan.centre[1],
        (a[1].min(b[1]) - 72.0 + a[1].max(b[1]) + 8.0) / 2.0
    );
    assert_eq!(plan.canvas, [640, 380]);
    // Standing still: the player's own box.
    let still = plan_camera(start, &[], [640, 380], 0.15);
    assert_eq!(still.centre, [a[0], a[1] - 32.0]);
    assert!(still.recentres.is_empty());
}

#[test]
fn a_long_walk_is_recentred_only_at_hop_boundaries_and_stays_inside() {
    let canvas = [640, 380];
    for margin in [0.0, 0.15, 0.3] {
        let hop = fit_steps(canvas, margin).min(10);
        let (start, hops) = row_walk(40, 160, hop);
        let plan = plan_camera(start, &hops, canvas, margin);
        assert!(!plan.recentres.is_empty(), "margin {margin}");
        let half = [
            (0.5 - margin) * f64::from(canvas[0]),
            (0.5 - margin) * f64::from(canvas[1]),
        ];
        // Replay the plan: every hop is inside the inner box of the centre
        // in effect while it is walked.
        let mut at = start;
        let mut current = plan.centre;
        let mut previous_hop = None;
        for (k, hop) in hops.iter().enumerate() {
            if let Some(r) = plan.recentres.iter().find(|r| r.before_hop == k) {
                assert_eq!(r.player_hex, at, "re-centred where the player stands");
                assert!(previous_hop.is_none_or(|p| p < k));
                previous_hop = Some(k);
                current = r.centre;
            }
            for &h in hop.path.iter().chain([&at]) {
                let p = centre(h);
                assert!(
                    p[0] - 16.0 >= current[0] - half[0] - 1e-9,
                    "margin {margin} hop {k}"
                );
                assert!(
                    p[0] + 16.0 <= current[0] + half[0] + 1e-9,
                    "margin {margin} hop {k}"
                );
                assert!(p[1] - 72.0 >= current[1] - half[1] - 1e-9);
                assert!(p[1] + 8.0 <= current[1] + half[1] + 1e-9);
            }
            at = hop.hex;
        }
    }
    // A wider margin re-centres more often.
    let count = |margin: f64| {
        let (start, hops) = row_walk(40, 160, 5);
        plan_camera(start, &hops, canvas, margin).recentres.len()
    };
    assert!(
        count(0.3) > count(0.05),
        "{} vs {}",
        count(0.3),
        count(0.05)
    );
    // The wide frame holds far more of the walk.
    let (start, hops) = row_walk(40, 160, 10);
    let wide = plan_camera(start, &hops, [1138, 507], 0.15).recentres.len();
    let classic = plan_camera(start, &hops, canvas, 0.15).recentres.len();
    assert!(wide < classic, "{wide} vs {classic}");
}

#[test]
fn a_hop_too_long_for_the_frame_is_centred_on_its_own() {
    let (start, hops) = row_walk(40, 140, 50);
    assert_eq!(hops.len(), 2);
    let plan = plan_camera(start, &hops, [640, 380], 0.15);
    // The first hop is framed by the first centre; the second gets one.
    assert_eq!(plan.recentres.len(), 1);
    assert_eq!(plan.recentres[0].before_hop, 1);
    assert_eq!(plan.recentres[0].player_hex, hex(90, 100));
    let mid = (centre(hex(90, 100))[0] + centre(hex(140, 100))[0]) / 2.0;
    assert!((plan.recentres[0].centre[0] - mid).abs() <= 1.0);
}

#[test]
fn stationary_hops_are_shortened_to_fit_the_frame() {
    assert_eq!(frame_size(View::Classic, 1.0), [640, 380]);
    assert_eq!(frame_size(View::Wide, 0.75), [1138, 507]);
    assert_eq!(fit_steps([640, 380], 0.15), 13);
    assert_eq!(fit_steps([1138, 507], 0.15), 22);
    assert_eq!(fit_steps([10, 10], 0.45), 1);
    let stationary = Framing::default();
    assert_eq!(stationary.camera(View::Classic), CameraKind::Stationary);
    assert_eq!(stationary.camera(View::Game), CameraKind::EngineFollowing);
    assert_eq!(framed_hop(15, &stationary, 0.75), 13);
    let follow = Framing {
        views: vec![View::Game, View::Wide],
        follow: true,
        margin: 0.15,
    };
    assert_eq!(follow.camera(View::Wide), CameraKind::Follows);
    assert_eq!(framed_hop(15, &follow, 0.75), 15);
    // Following or not, the same hops.
    let classic_follow = Framing {
        views: vec![View::Classic],
        follow: true,
        margin: 0.15,
    };
    assert_eq!(framed_hop(15, &classic_follow, 0.75), 13);
    let game = Framing {
        views: vec![View::Game],
        ..Framing::default()
    };
    assert_eq!(framed_hop(15, &game, 0.75), 15);
    let wide = Framing {
        views: vec![View::Wide],
        ..Framing::default()
    };
    assert_eq!(framed_hop(15, &wide, 0.75), 15);
    assert_eq!(
        framed_hop(15, &wide, 3.0),
        fit_steps(frame_size(View::Wide, 3.0), 0.15)
    );
}

#[test]
fn route_hops_carry_their_stretch_of_the_route() {
    let (start, hops) = row_walk(100, 132, 15);
    assert_eq!(start, hex(100, 100));
    let ends: Vec<i32> = hops.iter().map(|h| h.hex).collect();
    assert_eq!(ends, [hex(115, 100), hex(130, 100), hex(132, 100)]);
    let mut at = start;
    for hop in &hops {
        assert_eq!(hop.path.first(), Some(&at));
        assert_eq!(hop.path.last(), Some(&hop.hex));
        assert_eq!(hop.path.len(), hop.steps + 1);
        assert_eq!(hop.ticks, hop_ticks(hop.steps));
        at = hop.hex;
    }
}
