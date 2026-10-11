use super::*;

fn band() -> Rect {
    Rect { x: 2, y: 3, w: 60, h: 9 }
}

fn busy() -> Load {
    Load {
        cores: vec![0.9, 0.1, 0.5, 1.0],
        cpu: 0.625,
        memory: Some(0.41),
        disk: Some((12.0 * 1024.0 * 1024.0, 300.0 * 1024.0)),
        net: Some((1.2 * 1024.0 * 1024.0, 80.0 * 1024.0)),
        temp: Some(61.0),
        switches: Some(48_000.0),
        interrupts: Some(9_000.0),
        processes: Some(312),
        threads: Some(4_300),
        swap: Some(0.12),
        packets: Some((2_500.0, 600.0)),
        cpu_power: Some(67.0),
        battery_draw: Some(20.0),
    }
}

/// `steps` frames of `dt` with the same input, from a new animation.
fn run(kind: Kind, load: &Load, steps: usize, dt: f64) -> Anim {
    let mut a = Anim::new(kind, 7);
    for i in 0..steps {
        a.step(dt, Input { load: load.clone(), outs: vec![i as u64 * 4000], messages: 0 });
    }
    a
}

/// Like `run`, with a message delivered at each step in `at`.
fn run_with_messages(kind: Kind, load: &Load, steps: usize, dt: f64, at: &[usize]) -> Anim {
    let mut a = Anim::new(kind, 7);
    for i in 0..steps {
        let messages = at.iter().filter(|&&s| s <= i).count() as u64;
        a.step(dt, Input { load: load.clone(), outs: vec![0], messages });
    }
    a
}

fn drawn(a: &Anim, band: Rect, pal: &Palette) -> Grid {
    let mut g = Grid::new(64, 14);
    a.draw(&mut g, band, pal);
    g
}

fn text(g: &Grid) -> String {
    (0..g.rows).map(|y| (0..g.cols).map(|x| g.get(x, y).text()).collect::<String>() + "\n").collect()
}

/// The same steps give the same frame, cell for cell, each kind; and
/// nothing is drawn outside the band.
#[test]
fn the_same_steps_give_the_same_frame_inside_the_band() {
    let pal = Palette::default();
    for kind in Kind::ALL {
        let (a, b) = (run(kind, &busy(), 90, 0.05), run(kind, &busy(), 90, 0.05));
        let (ga, gb) = (drawn(&a, band(), &pal), drawn(&b, band(), &pal));
        assert_eq!(ga, gb, "{kind:?}");
        let b = band();
        let outside = (0..ga.rows)
            .flat_map(|y| (0..ga.cols).map(move |x| (x, y)))
            .filter(|&(x, y)| x < b.x || x >= b.x + b.w || y < b.y || y >= b.y + b.h);
        for (x, y) in outside {
            assert_eq!(ga.get(x, y), Grid::new(64, 14).get(x, y), "{kind:?} drew at ({x}, {y})");
        }
        assert_ne!(text(&ga).trim(), "", "{kind:?} drew something");
    }
}

/// Every size from nothing to wide, each kind, with a machine that
/// gives everything, nothing, 64 cores, or figures out of range: no
/// panic, nothing drawn outside the band.
#[test]
fn every_size_draws_inside_its_band() {
    let wild = Load {
        cores: vec![2.0, -1.0],
        cpu: f32::NAN,
        memory: Some(7.0),
        disk: Some((f64::INFINITY, -5.0)),
        net: Some((1e300, 0.0)),
        temp: Some(f32::NAN),
        switches: Some(f64::NAN),
        interrupts: Some(f64::INFINITY),
        processes: Some(u64::MAX),
        threads: Some(0),
        swap: Some(-3.0),
        packets: Some((f64::NAN, -1.0)),
        cpu_power: Some(f32::INFINITY),
        battery_draw: Some(f32::NAN),
    };
    let loads = [busy(), Load::default(), Load { cores: vec![0.7; 64], cpu: 0.7, ..busy() }, wild];
    let mut checked = 0;
    for kind in Kind::ALL {
        for load in &loads {
            let a = run(kind, load, 30, 0.05);
            for w in (0..=130).step_by(7) {
                for h in 0..=13 {
                    let band = Rect { x: 1, y: 1, w, h };
                    let mut g = Grid::new(132, 15);
                    a.draw(&mut g, band, &Palette::default());
                    for y in 0..g.rows {
                        for x in 0..g.cols {
                            let inside = x >= band.x && x < band.x + w && y >= band.y && y < band.y + h;
                            assert!(inside || g.get(x, y).text() == " ", "{kind:?} {w}x{h} drew at ({x}, {y})");
                        }
                    }
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, Kind::ALL.len() * 4 * 19 * 14);
}

/// Too little room: nothing at all.
#[test]
fn a_band_too_small_is_left_alone() {
    let pal = Palette::default();
    for kind in Kind::ALL {
        let a = run(kind, &busy(), 40, 0.05);
        for small in [Rect { w: MIN_W - 1, ..band() }, Rect { h: MIN_H - 1, ..band() }] {
            assert_eq!(drawn(&a, small, &pal), Grid::new(64, 14), "{kind:?} {small:?}");
        }
        assert_ne!(drawn(&a, Rect { w: MIN_W, h: MIN_H, ..band() }, &pal), Grid::new(64, 14), "{kind:?} at the least");
    }
}

/// The heart follows the cores: 60 beats a minute idle, 150 all busy;
/// six seconds hold 6 and 15 beats.
#[test]
fn the_heart_beats_with_the_cpu() {
    let beats = |cpu: f32| {
        let a = run(Kind::Pulse, &Load { cpu, ..Load::default() }, 120, 0.05);
        let s: Vec<f32> = a.samples.iter().map(|s| s[0]).collect();
        s.windows(2).filter(|w| w[0] < 0.5 && w[1] >= 0.5).count()
    };
    assert_eq!(beats(0.0), 6);
    assert_eq!(beats(1.0), 15);
    assert_eq!(run(Kind::Pulse, &Load::default(), 120, 0.05).samples.len(), 240, "40 samples a second");
}

/// The traces follow the machine: no network or disk figure, no trace
/// for it; their labels say the figures; a quiet network draws flat.
#[test]
fn the_monitor_shows_what_the_machine_gives() {
    let pal = Palette::default();
    let t = text(&drawn(&run(Kind::Pulse, &busy(), 100, 0.05), band(), &pal));
    assert!(t.contains("CPU 63%") && t.contains("61°C"), "{t}");
    assert!(t.contains("NET ↓1.2M ↑80K") && t.contains("DISK r12M w300K"), "{t}");
    let bare = Load { cpu: 0.2, ..Load::default() };
    let t = text(&drawn(&run(Kind::Pulse, &bare, 100, 0.05), band(), &pal));
    assert!(t.contains("CPU 20%") && !t.contains("NET") && !t.contains("DISK") && !t.contains("°C"), "{t}");
    // The network's trace: its tallest point by how much goes through.
    let span = |net: f64| {
        let a = run(Kind::Pulse, &Load { net: Some((net, 0.0)), ..Load::default() }, 100, 0.05);
        a.samples.iter().map(|s| s[1]).fold(0.0, f32::max)
    };
    assert!(span(0.0) < 0.1 && span(100.0 * 1024.0 * 1024.0) > 0.7, "{} {}", span(0.0), span(1e8));
}

/// The disks move both: the monitor's breath comes faster and deeper,
/// the rain's characters change quicker.
#[test]
fn the_disks_quicken_the_breath_and_the_flicker() {
    let disk = |rate: f64| Load { disk: Some((rate, 0.0)), ..Load::default() };
    let breaths = |load: &Load| {
        let a = run(Kind::Pulse, load, 240, 0.05);
        let s: Vec<f32> = a.samples.iter().map(|s| s[2]).collect();
        let deepest = s.iter().copied().fold(0.0, f32::max);
        (s.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count(), deepest)
    };
    // 12 s: one breath in 5 s with the disks idle, one in 1.4 s busy.
    let (calm, shallow) = breaths(&disk(0.0));
    let (busy, deep) = breaths(&disk(1e9));
    assert_eq!((calm, busy), (2, 8), "breaths in 12 s");
    assert!(shallow < 0.1 && deep > 0.95, "{shallow} {deep}");
    let flicker = |load: &Load| run(Kind::Matrix, load, 50, 0.08).flicker;
    assert!(flicker(&disk(1e9)) > flicker(&disk(0.0)) * 5.0, "{} {}", flicker(&disk(0.0)), flicker(&disk(1e9)));
}

/// Green, yellow, red: by the temperature when there is one (whatever
/// the cores do), else by the cores; the heart's trace and the rain take
/// it.
#[test]
fn the_colour_follows_the_temperature_else_the_cores() {
    let at = |temp: Option<f32>, cpu: f32| heat(&Load { temp, cpu, ..Load::default() });
    assert_eq!((at(Some(45.0), 1.0), at(Some(70.0), 0.0), at(Some(85.0), 0.0)), (GREEN, YELLOW, RED));
    assert_eq!((at(None, 0.3), at(None, 0.6), at(None, 0.95)), (GREEN, YELLOW, RED));
    let pal = Palette::default();
    let colours = |kind: Kind, temp: f32| {
        let g =
            drawn(&run(kind, &Load { temp: Some(temp), memory: Some(1.0), ..Load::default() }, 80, 0.05), band(), &pal);
        (0..g.rows)
            .flat_map(|y| (0..g.cols).map(move |x| (x, y)))
            .map(|(x, y)| g.get(x, y).style.fg)
            .collect::<Vec<_>>()
    };
    for kind in [Kind::Pulse, Kind::Matrix] {
        let (cool, hot) = (colours(kind, 40.0), colours(kind, 90.0));
        assert!(cool.contains(&Color::Idx(GREEN)) && !cool.contains(&Color::Idx(RED)), "{kind:?} cool");
        assert!(hot.contains(&Color::Idx(RED)) && !hot.contains(&Color::Idx(GREEN)), "{kind:?} hot");
    }
}

/// The rain: more columns the more memory is used, faster the busier
/// the cores, cyan columns with the network.
#[test]
fn the_rain_follows_memory_cores_and_network() {
    let pal = Palette::default();
    let lit = |load: &Load| {
        let g = drawn(&run(Kind::Matrix, load, 60, 0.08), band(), &pal);
        (0..g.rows).flat_map(|y| (0..g.cols).map(move |x| (x, y))).filter(|&(x, y)| g.get(x, y).text() != " ").count()
    };
    let few = lit(&Load { memory: Some(0.0), ..Load::default() });
    let many = lit(&Load { memory: Some(1.0), ..Load::default() });
    assert!(many > few * 2, "{few} {many}");
    let fell = |cpu: f32| run(Kind::Matrix, &Load { cpu, ..Load::default() }, 50, 0.08).travel;
    assert!(fell(1.0) > fell(0.0) * 3.0, "{} {}", fell(0.0), fell(1.0));
    let cyan = |load: &Load| {
        let g = drawn(&run(Kind::Matrix, load, 60, 0.08), Rect { x: 0, y: 0, w: 64, h: 14 }, &pal);
        (0..g.rows)
            .flat_map(|y| (0..g.cols).map(move |x| (x, y)))
            .filter(|&(x, y)| g.get(x, y).style.fg == Color::Idx(CYAN))
            .count()
    };
    let quiet = Load { memory: Some(1.0), net: Some((0.0, 0.0)), ..Load::default() };
    let loud = Load { net: Some((1e9, 1e9)), ..quiet.clone() };
    assert!(cyan(&loud) > cyan(&quiet) * 3, "{} {}", cyan(&quiet), cyan(&loud));
}

/// The equalizer: a full core is a full bar, an idle one empty; each
/// group labelled; what the machine does not give, left out; too
/// narrow, the cores become one bar.
#[test]
fn the_bars_stand_for_each_figure() {
    let pal = Palette::default();
    let load = Load { cores: vec![1.0, 0.0], cpu: 0.5, ..Load::default() };
    let a = run(Kind::Bars, &load, 60, 0.066);
    assert!(a.bars[0].h > 0.95 && a.bars[1].h < 0.05, "{:?}", a.bars);
    let g = drawn(&a, band(), &pal);
    let t = text(&g);
    assert!(t.contains("CPU 50%") && t.contains("OUT ") && !t.contains("MEM") && !t.contains("NET"), "{t}");
    let top: Vec<u16> = (0..g.cols).filter(|&x| g.get(x, band().y).text() != " ").collect();
    assert_eq!(top.len(), 2, "one wide bar reaches the top row: {t}");
    // Each label is given room for its figures at their widest: the six
    // first groups need 66 columns here, and in 60 the last is left out.
    let busy_bars = run(Kind::Bars, &busy(), 60, 0.066);
    let mut g = Grid::new(70, 9);
    busy_bars.draw(&mut g, Rect { x: 0, y: 0, w: 66, h: 9 }, &pal);
    let t = text(&g);
    for label in ["CPU 63%", "MEM 41%", "DISK 12M", "NET ↓1.2M ↑80K", "61°C", "OUT "] {
        assert!(t.contains(label), "{label}: {t}");
    }
    let t = text(&drawn(&busy_bars, band(), &pal));
    assert!(t.contains("61°C") && !t.contains("OUT "), "{t}");
    // A bar that falls leaves its peak's mark over it a while; an idle
    // one has none, on the bottom row or anywhere.
    let full = Load { cores: vec![1.0], cpu: 1.0, ..Load::default() };
    let mut a = run(Kind::Bars, &full, 30, 0.066);
    let idle = Load { cores: vec![0.0], ..Load::default() };
    for _ in 0..5 {
        a.step(0.066, Input { load: idle.clone(), outs: vec![0], messages: 0 });
    }
    let g = drawn(&a, band(), &pal);
    assert!((0..g.cols).any(|x| g.get(x, band().y).text() == "▔"), "{}", text(&g));
    let g = drawn(&run(Kind::Bars, &idle, 30, 0.066), band(), &pal);
    assert!(!text(&g).contains('▔'), "{}", text(&g));
    // 32 cores in 40 columns: taken four together, 8 bars with a gap
    // between each (never a solid block), the other groups still there.
    let many = Load { cores: vec![0.5; 32], cpu: 0.5, memory: Some(0.3), ..Load::default() };
    let a = run(Kind::Bars, &many, 30, 0.066);
    let narrow = Rect { x: 0, y: 0, w: 40, h: 6 };
    let g = drawn(&a, narrow, &pal);
    let t = text(&g);
    assert!(t.contains("CPU 50%") && t.contains("MEM 30%") && t.contains("OUT"), "{t}");
    let bottom: String = (0..g.cols).map(|x| g.get(x, 4).text()).collect();
    let runs: Vec<usize> = bottom.split(' ').filter(|r| !r.is_empty()).map(|r| r.chars().count()).collect();
    assert_eq!(runs, vec![1; 10], "8 core bars, memory, output, each alone (no message: no bar): {t}");
    assert!(!t.contains("MSG"), "room for the messages' label at its widest (43 columns): none: {t}");
    // Cores taken together stand at their average, not their highest:
    // half of them full and half idle make bars half way up.
    let half = Load { cores: (0..32).map(|i| (i % 2) as f32).collect(), cpu: 0.5, ..many.clone() };
    let g = drawn(&run(Kind::Bars, &half, 30, 0.066), Rect { h: 10, ..narrow }, &pal);
    let rows: Vec<String> = (0..9).map(|y| (0..22).map(|x| g.get(x, y).text()).collect()).collect();
    assert!(rows[0].trim().is_empty() && rows[1].trim().is_empty(), "no core bar near the top: {rows:#?}");
    assert!(rows[8].contains('█') && rows[5].contains('█'), "half way up: {rows:#?}");
}

/// The groups added after the first ones (messages, watts, the battery,
/// swap, context switches) are drawn where there is room for them
/// beside those, in the same look; where there is not, the frame is
/// the one a machine without those figures gives, cell for cell.
#[test]
fn the_added_bars_take_only_the_room_left() {
    let pal = Palette::default();
    let old = Load {
        switches: None,
        interrupts: None,
        processes: None,
        threads: None,
        swap: None,
        packets: None,
        cpu_power: None,
        battery_draw: None,
        ..busy()
    };
    let wide = Rect { x: 0, y: 0, w: 150, h: 9 };
    let mut g = Grid::new(150, 9);
    run(Kind::Bars, &busy(), 60, 0.066).draw(&mut g, wide, &pal);
    let t = text(&g);
    for label in ["CPU 63%", "OUT ", "67W", "PWR 20W", "SWAP 12%", "CTX 48k", "MSG 0"] {
        assert!(t.contains(label), "{label}: {t}");
    }
    let at = |load: &Load, w: u16| {
        let mut g = Grid::new(150, 9);
        run(Kind::Bars, load, 60, 0.066).draw(&mut g, Rect { w, ..wide }, &pal);
        g
    };
    let (mut same, mut more) = (0, 0);
    for w in (20..=150).step_by(2) {
        let (a, b) = (at(&old, w), at(&busy(), w));
        if a == b {
            same += 1;
        } else {
            // Where they differ, the old frame's every group is still
            // there, as many bars wide; the new one only adds to it.
            more += 1;
            let (ta, tb) = (text(&a), text(&b));
            for label in ["CPU 63%", "MEM 41%", "DISK 12M", "NET ↓1.2M ↑80K", "61°C", "OUT "] {
                assert_eq!(ta.contains(label), tb.contains(label), "{w} {label}: {ta}\n{tb}");
            }
            let lit = |g: &Grid| (0..g.cols).filter(|&x| g.get(x, 7).text() != " ").count();
            assert!(lit(&b) >= lit(&a), "{w}: {ta}\n{tb}");
        }
    }
    assert!(same > 0 && more > 0, "{same} the same, {more} with more");
}

/// The layout holds still while the figures change: the same machine
/// with every figure short ("0", "1%") and every figure at its widest
/// draws its groups and traces in the same columns, the same of them.
#[test]
fn the_layout_holds_while_the_figures_change() {
    let pal = Palette::default();
    let short = Load {
        cores: vec![0.01; 8],
        cpu: 0.01,
        memory: Some(0.01),
        disk: Some((0.0, 0.0)),
        net: Some((0.0, 0.0)),
        temp: Some(5.0),
        switches: Some(1.0),
        interrupts: Some(1.0),
        processes: Some(9),
        threads: Some(9),
        swap: Some(0.01),
        packets: Some((0.0, 0.0)),
        cpu_power: Some(1.0),
        battery_draw: Some(1.0),
    };
    let wide = Load {
        cores: vec![1.0; 8],
        cpu: 1.0,
        memory: Some(1.0),
        disk: Some((999.0 * 1024.0, 999.0 * 1024.0)),
        net: Some((9.9 * 1024.0 * 1024.0, 999.0 * 1024.0)),
        temp: Some(99.0),
        switches: Some(999_000.0),
        interrupts: Some(999_000.0),
        processes: Some(9999),
        threads: Some(999_000),
        swap: Some(1.0),
        packets: Some((500_000.0, 499_000.0)),
        cpu_power: Some(999.0),
        battery_draw: Some(999.0),
    };
    // Where the label column starts in pulse (the first column of the
    // heart's label), and which groups the bars show, by their words.
    let pulse_at = |load: &Load, w: u16| {
        let mut g = Grid::new(140, 14);
        run(Kind::Pulse, load, 40, 0.05).draw(&mut g, Rect { x: 0, y: 0, w, h: 13 }, &pal);
        (0..g.rows).find_map(|y| (0..g.cols).find(|&x| g.get(x, y).text() == "♥"))
    };
    // The groups' words, and the columns of the cores' bars (both
    // machines' cores half busy).
    let bars = |load: &Load, w: u16| {
        let load = Load { cores: vec![0.5; 8], cpu: 0.5, ..load.clone() };
        let mut g = Grid::new(140, 9);
        run(Kind::Bars, &load, 40, 0.066).draw(&mut g, Rect { x: 0, y: 0, w, h: 9 }, &pal);
        // Each label: its word, and the column of its middle.
        let row: Vec<String> = (0..g.cols).map(|x| g.get(x, 8).text().to_string()).collect();
        let (mut labels, mut x) = (Vec::new(), 0);
        while x < row.len() {
            if row[x] == " " {
                x += 1;
                continue;
            }
            let start = x;
            while x < row.len() && !(row[x] == " " && row.get(x + 1).is_none_or(|c| c == " ")) {
                x += 1;
            }
            let word: String = row[start..x].concat().chars().take_while(|c| c.is_alphabetic()).collect();
            labels.push((word, (start + x) / 2));
        }
        // The cores' group ends where memory's label begins.
        let mem = (0..g.cols).find(|&x| g.get(x, 8).text() == "M").unwrap_or(g.cols);
        let cores: Vec<u16> = (0..mem).filter(|&x| g.get(x, 7).text() != " ").collect();
        (labels, cores)
    };
    let mut checked = 0;
    for w in (20..=140).step_by(3) {
        assert_eq!(pulse_at(&short, w), pulse_at(&wide, w), "pulse at {w}");
        let ((a, ca), (b, cb)) = (bars(&short, w), bars(&wide, w));
        assert_eq!(ca, cb, "the cores' bars at {w}");
        assert_eq!(a.len(), b.len(), "bars at {w}: {a:?} {b:?}");
        for ((wa, xa), (wb, xb)) in a.iter().zip(&b) {
            assert!(wa == wb && xa.abs_diff(*xb) <= 1, "bars at {w}: {a:?} {b:?}");
        }
        checked += 1;
    }
    assert_eq!(checked, 41);
}

/// The monitor as it was where the new figures have no room: the
/// heart, network and disk keep their rows (the system's wave only where
/// a row is left for it), and their short labels where the full ones do
/// not fit. Every size: a frame with neither the system's wave nor a
/// full label is the one a machine without those figures gives.
#[test]
fn the_monitor_keeps_its_rows_and_labels_where_the_new_figures_do_not_fit() {
    let pal = Palette::default();
    let old = Load {
        switches: None,
        interrupts: None,
        processes: None,
        threads: None,
        swap: None,
        packets: None,
        cpu_power: None,
        battery_draw: None,
        ..busy()
    };
    let (a, b) = (run(Kind::Pulse, &old, 90, 0.05), run(Kind::Pulse, &busy(), 90, 0.05));
    let (mut same, mut short, mut full) = (0, 0, 0);
    for w in (20..=130).step_by(2) {
        for h in 3..=13 {
            let r = Rect { x: 0, y: 0, w, h };
            let (mut ga, mut gb) = (Grid::new(132, 14), Grid::new(132, 14));
            a.draw(&mut ga, r, &pal);
            b.draw(&mut gb, r, &pal);
            let tb = text(&gb);
            // The system's wave has a row of its own: a fourth trace
            // in rows of 3 (9 rows and more) or of 2.
            let wave = h / if h >= 9 { 3 } else { 2 } >= 4;
            if wave || tb.contains("67W") {
                full += 1;
                continue;
            }
            assert_eq!(ga, gb, "{w}x{h}:\n{}\n{tb}", text(&ga));
            same += 1;
            short += usize::from(tb.contains("CPU 63%  61°C"));
        }
    }
    assert!(same > 0 && short > 0 && full > 0, "{same} the same ({short} with short labels), {full} with more");
    // 9 rows: three traces of 3 rows each, as before; 12: the system's
    // wave under them.
    let rows_of = |h: u16| {
        let mut g = Grid::new(132, 14);
        b.draw(&mut g, Rect { x: 0, y: 0, w: 120, h }, &pal);
        text(&g).contains("SYS")
    };
    assert!(!rows_of(9) && !rows_of(11) && rows_of(12));
}

/// A message delivered: the heart beats early (more beats in the same
/// time) and the count shows by it; a drop headed by an envelope falls
/// through the rain; the messages' bar fills and falls back. Messages
/// delivered before the clock came are not counted.
#[test]
fn a_message_shows_in_each_kind() {
    let pal = Palette::default();
    let beats = |a: &Anim| {
        let s: Vec<f32> = a.samples.iter().map(|s| s[0]).collect();
        s.windows(2).filter(|w| w[0] < 0.5 && w[1] >= 0.5).count()
    };
    let quiet = run_with_messages(Kind::Pulse, &Load::default(), 120, 0.05, &[]);
    let told = run_with_messages(Kind::Pulse, &Load::default(), 120, 0.05, &[20, 45, 70, 95]);
    assert!(beats(&told) > beats(&quiet), "{} {}", beats(&quiet), beats(&told));
    assert_eq!(told.messages, 4);
    assert!(text(&drawn(&told, band(), &pal)).contains("✉ 4"));
    assert!(!text(&drawn(&quiet, band(), &pal)).contains('✉'));
    // Five came before the clock: none counted.
    let mut a = Anim::new(Kind::Pulse, 7);
    for i in 0..10 {
        a.step(0.05, Input { messages: 5 + u64::from(i >= 5), ..Input::default() });
    }
    assert_eq!(a.messages, 1);
    // The rain: an envelope at the head of the drop, 4 steps (0.32 s)
    // after.
    let rain = run_with_messages(Kind::Matrix, &Load::default(), 60, 0.08, &[55]);
    let g = drawn(&rain, band(), &pal);
    let envelopes: Vec<(u16, u16)> = (0..g.rows)
        .flat_map(|y| (0..g.cols).map(move |x| (x, y)))
        .filter(|&(x, y)| g.get(x, y).text() == "✉")
        .collect();
    assert_eq!(envelopes.len(), 1, "{}", text(&g));
    assert_eq!(envelopes[0].1, band().y + 5, "18 rows a second, 0.32 s");
    assert!(g.get(envelopes[0].0, envelopes[0].1).style.bold);
    assert!(!text(&drawn(&run(Kind::Matrix, &Load::default(), 60, 0.08), band(), &pal)).contains('✉'));
    // Gone below the band, and let go a while after.
    let later = run_with_messages(Kind::Matrix, &Load::default(), 400, 0.08, &[55]);
    assert!(!text(&drawn(&later, band(), &pal)).contains('✉') && later.drops.is_empty());
    // A thousand at once: all counted, a few drops and early beats, not
    // a thousand; drawn in every kind.
    for kind in Kind::ALL {
        let mut a = Anim::new(kind, 7);
        for i in 0..40u64 {
            a.step(0.05, Input { messages: if i >= 20 { 1000 } else { 0 }, ..Input::default() });
        }
        assert_eq!(a.messages, 1000);
        assert!(a.drops.len() <= DROPS && a.early_beats <= 4, "{} {}", a.drops.len(), a.early_beats);
        let t = text(&drawn(&a, band(), &pal));
        assert!(t.contains(if kind == Kind::Bars { "MSG 1000" } else { "" }), "{t}");
    }
    assert!(text(&drawn(&run(Kind::Pulse, &busy(), 30, 0.05), band(), &pal)).contains("♥"));
    // No pane's output given (the pane's window not found): one bar,
    // at nothing.
    let mut a = Anim::new(Kind::Bars, 7);
    a.step(0.05, Input::default());
    a.step(0.05, Input::default());
    let (groups, _) = a.groups();
    assert!(groups.iter().any(|g| g.label == "OUT 0" && g.levels == vec![0.0]), "{groups:?}");
    // The bar: full at once, under a tenth 2 s on.
    let bar = |steps: usize| run_with_messages(Kind::Bars, &Load::default(), steps, 0.05, &[20]).message_level;
    assert!(bar(21) > 0.9 && bar(60) < 0.1, "{} {}", bar(21), bar(60));
    assert!(
        text(&drawn(&run_with_messages(Kind::Bars, &Load::default(), 30, 0.05, &[20]), band(), &pal)).contains("MSG 1")
    );
}

/// The system's wave: more swings the more the CPUs switch, taller the
/// more interrupts come; its label says the switches, processes and
/// threads. The rain's stray characters come with the interrupts.
#[test]
fn the_system_wave_and_the_strays_follow_switches_and_interrupts() {
    let pal = Palette::default();
    let sys = |switches: f64, interrupts: f64| Load {
        switches: Some(switches),
        interrupts: Some(interrupts),
        ..Load::default()
    };
    let wave = |load: &Load| {
        let a = run(Kind::Pulse, load, 120, 0.05);
        let s: Vec<f32> = a.samples.iter().map(|s| s[3]).collect();
        let turns = s.windows(3).filter(|w| (w[1] - w[0]) * (w[2] - w[1]) < 0.0).count();
        (turns, s.iter().map(|v| v.abs()).fold(0.0, f32::max))
    };
    let (calm, low) = wave(&sys(1e3, 1e3));
    let (busy_turns, _) = wave(&sys(1e6, 1e3));
    let (_, high) = wave(&sys(1e3, 1e6));
    assert!(busy_turns > calm * 3, "{calm} {busy_turns}");
    assert!(high > low * 3.0, "{low} {high}");
    let t = text(&drawn(&run(Kind::Pulse, &busy(), 60, 0.05), Rect { x: 0, y: 0, w: 64, h: 14 }, &pal));
    assert!(t.contains("SYS 48k/s  312 proc 4.3k thr"), "{t}");
    assert!(t.contains("CPU 63% 67W") && t.contains("PWR 20W") && t.contains("NET ↓1.2M ↑80K  3.1k pkt/s"), "{t}");
    assert!(!text(&drawn(&run(Kind::Pulse, &Load::default(), 60, 0.05), band(), &pal)).contains("SYS"));
    let strays = |load: &Load| {
        let g = drawn(&run(Kind::Matrix, load, 60, 0.08), band(), &pal);
        (0..g.rows)
            .flat_map(|y| (0..g.cols).map(move |x| (x, y)))
            .filter(|&(x, y)| g.get(x, y).style.fg == Color::Idx(MAGENTA))
            .count()
    };
    assert_eq!(strays(&sys(1e3, 1e3)), 0);
    assert!(strays(&sys(1e3, 1e6)) > 10, "{}", strays(&sys(1e3, 1e6)));
}

/// The output bars: one a pane of the window, each at its own rate,
/// the label their sum; a pane more or fewer starts them again.
#[test]
fn each_pane_has_its_output_bar() {
    let mut a = Anim::new(Kind::Bars, 7);
    for i in 0..40u64 {
        a.step(0.05, Input { outs: vec![100, i * 50_000, 0], ..Input::default() });
    }
    assert_eq!(a.out_rates.len(), 3);
    assert!(a.out_rates[0] == 0.0 && a.out_rates[2] == 0.0, "{:?}", a.out_rates);
    assert!((a.out_rates[1] - 1e6).abs() < 1e4, "1 MB a second: {:?}", a.out_rates);
    let (groups, first) = a.groups();
    let out = groups.iter().find(|g| g.label.starts_with("OUT")).unwrap();
    assert_eq!(out.label, format!("OUT {}", rate_text(a.out_rates[1])));
    assert!(out.label.starts_with("OUT 9") && out.label.ends_with('K'), "{}", out.label);
    assert!(out.levels.len() == 3 && out.levels[1] > 0.8 && out.levels[0] == 0.0, "{out:?}");
    assert_eq!(groups[first - 1].label, out.label, "the output is the last of the first groups");
    a.step(0.05, Input { outs: vec![100, 2_000_000], ..Input::default() });
    assert_eq!(a.out_rates, vec![0.0, 0.0], "a pane fewer: from nothing");
}

/// Colours are the theme's: the 16 by index (the theme's palette maps
/// them); a fading tail mixes into the background when both are RGB,
/// else it is the same colour, dim.
#[test]
fn the_colours_are_the_themes() {
    let none = Palette::default();
    assert_eq!(none.shade(GREEN, 0.0), Style::colors(Color::Idx(GREEN), Color::Default));
    assert!(none.shade(GREEN, 0.7).dim && !none.shade(GREEN, 0.2).dim);
    let mut colours = vec![Color::Idx(0); 16];
    colours[2] = Color::Rgb(0x9e, 0xce, 0x6a);
    let night = Palette { colours, bg: Color::Rgb(0x1a, 0x1b, 0x26) };
    assert_eq!(night.shade(GREEN, 0.0).fg, Color::Idx(GREEN), "unfaded: the theme colours it");
    assert_eq!(night.shade(GREEN, 0.5).fg, Color::Rgb(0x5c, 0x75, 0x48), "half way to the background");
    assert_eq!(night.shade(GREEN, 1.0).fg, Color::Rgb(0x1a, 0x1b, 0x26));
    // What an animation draws: only the 16, or mixes of them.
    for kind in Kind::ALL {
        let g = drawn(&run(kind, &busy(), 60, 0.05), band(), &night);
        for y in 0..g.rows {
            for x in 0..g.cols {
                let fg = g.get(x, y).style.fg;
                assert!(matches!(fg, Color::Default | Color::Idx(0..=15) | Color::Rgb(..)), "{kind:?} {fg:?}");
            }
        }
    }
}

#[test]
fn rates_and_levels_read_short() {
    assert_eq!(rate_text(0.0), "0");
    assert_eq!(rate_text(812.0), "812");
    assert_eq!(rate_text(12.0 * 1024.0), "12K");
    assert_eq!(rate_text(1.2 * 1024.0 * 1024.0), "1.2M");
    assert_eq!(rate_text(5e12), "4657G");
    assert_eq!((bytes_level(0.0), bytes_level(1e3), bytes_level(1e9), bytes_level(1e12)), (0.0, 0.0, 1.0, 1.0));
    assert_eq!(out_level(10.0), 0.0);
    assert!((bytes_level(1e6) - 0.5).abs() < 1e-6);
}

/// The whole 64 x 14 grid as the band.
fn full() -> Rect {
    Rect { x: 0, y: 0, w: 64, h: 14 }
}

/// How many cells of `g` `f` holds for.
fn cells(g: &Grid, f: impl Fn(&Cell) -> bool) -> usize {
    (0..g.rows).flat_map(|y| (0..g.cols).map(move |x| (x, y))).filter(|&(x, y)| f(g.get(x, y))).count()
}

/// Where the envelopes are.
fn envelopes(g: &Grid) -> Vec<(u16, u16)> {
    (0..g.rows)
        .flat_map(|y| (0..g.cols).map(move |x| (x, y)))
        .filter(|&(x, y)| g.get(x, y).text() == ENVELOPE)
        .collect()
}

/// Each kind by its name, and back; random: never the kind before, every
/// kind reached from each, about as often as each other.
#[test]
fn a_kind_by_its_name_and_at_random() {
    for kind in Kind::ALL {
        assert_eq!(Kind::named(kind.name()), Some(kind));
    }
    assert_eq!(Kind::named("cycle"), None);
    assert_eq!(Kind::named("off"), None);
    for previous in Kind::ALL.map(Some).into_iter().chain([None]) {
        let mut seen = [0usize; Kind::ALL.len()];
        for r in 0..9000u64 {
            let k = Kind::random(previous, r);
            assert_ne!(Some(k), previous);
            seen[Kind::ALL.iter().position(|&x| x == k).unwrap()] += 1;
        }
        let each = 9000 / if previous.is_some() { Kind::ALL.len() - 1 } else { Kind::ALL.len() };
        for (i, &n) in seen.iter().enumerate() {
            if previous == Some(Kind::ALL[i]) {
                assert_eq!(n, 0);
            } else {
                assert!(n > each * 8 / 10 && n < each * 12 / 10, "{previous:?}: {seen:?}");
            }
        }
    }
}

/// The flames: a busy core's taller than an idle one's; the tip white only
/// when hot; sparks over them with the interrupts; a message's envelope
/// rises through them, 6 rows a second.
#[test]
fn the_fire_follows_the_cores_the_heat_and_the_interrupts() {
    let pal = Palette::default();
    let load = Load { cores: vec![1.0, 0.0], cpu: 0.5, ..Load::default() };
    let g = drawn(&run(Kind::Fire, &load, 40, 0.066), full(), &pal);
    let lit = |xs: std::ops::Range<u16>| {
        xs.flat_map(|x| (0..14).map(move |y| (x, y))).filter(|&(x, y)| g.get(x, y).text() != " ").count()
    };
    let (busy_side, idle_side) = (lit(0..16), lit(48..64));
    assert!(busy_side > idle_side * 2, "{busy_side} {idle_side}\n{}", text(&g));
    let white = |temp: f32| {
        let load = Load { cores: vec![1.0; 4], cpu: 1.0, temp: Some(temp), ..Load::default() };
        cells(&drawn(&run(Kind::Fire, &load, 40, 0.066), full(), &pal), |c| c.style.fg == Color::Idx(BRIGHT_WHITE))
    };
    assert!(white(90.0) > 10 && white(40.0) == 0, "{} {}", white(90.0), white(40.0));
    // Idle cores keep the flames low: the top 8 rows hold only sparks.
    let sparks = |interrupts: f64| {
        let load = Load { cores: vec![0.0; 4], interrupts: Some(interrupts), ..Load::default() };
        let g = drawn(&run(Kind::Fire, &load, 40, 0.066), full(), &pal);
        (0..8).flat_map(|y| (0..64).map(move |x| (x, y))).filter(|&(x, y)| g.get(x, y).text() != " ").count()
    };
    assert!(sparks(1e3) == 0 && sparks(1e6) > 5, "{} {}", sparks(1e3), sparks(1e6));
    // 9 steps (0.594 s) after: 3 rows up from the bottom.
    let a = run_with_messages(Kind::Fire, &Load::default(), 60, 0.066, &[51]);
    let e = envelopes(&drawn(&a, band(), &pal));
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].1, band().y + band().h - 1 - 3);
    let a = run_with_messages(Kind::Fire, &Load::default(), 120, 0.066, &[51]);
    assert!(envelopes(&drawn(&a, band(), &pal)).is_empty(), "risen out of the band");
}

/// The tank: a fish a pane, each faster the more its pane prints; bubbles
/// with the disks; weed taller the more memory is used; a message's
/// envelope rises from the sand.
#[test]
fn the_aquarium_has_a_fish_a_pane() {
    let pal = Palette::default();
    let swim = |outs: &dyn Fn(u64) -> Vec<u64>| {
        let mut a = Anim::new(Kind::Aquarium, 7);
        for i in 0..40u64 {
            a.step(0.1, Input { outs: outs(i), ..Input::default() });
        }
        a
    };
    let a = swim(&|i| vec![0, i * 100_000, 0]);
    assert_eq!(a.tank.swum.len(), 3);
    assert!(a.tank.swum[1] > a.tank.swum[0] * 5.0, "{:?}", a.tank.swum);
    let angles = |a: &Anim| cells(&drawn(a, full(), &pal), |c| c.text() == "<" || c.text() == ">");
    // "><>", "><(((o>", ">=>": 3, 3 and 2 marks.
    assert_eq!(angles(&a), 8, "{}", text(&drawn(&a, full(), &pal)));
    assert_eq!(angles(&swim(&|_| vec![0])), 3);
    let one = |load: Load| run(Kind::Aquarium, &load, 40, 0.1);
    let bubbles = |disk: f64| {
        let g = drawn(&one(Load { disk: Some((disk, 0.0)), ..Load::default() }), full(), &pal);
        cells(&g, |c| c.text() == "o" || c.text() == "O")
    };
    assert!(bubbles(0.0) == 0 && bubbles(1e9) > 5, "{} {}", bubbles(0.0), bubbles(1e9));
    let weed = |memory: f32| {
        let g = drawn(&one(Load { memory: Some(memory), ..Load::default() }), full(), &pal);
        cells(&g, |c| c.text() == "(" || c.text() == ")")
    };
    assert!(weed(1.0) > weed(0.0) * 3, "{} {}", weed(0.0), weed(1.0));
    let a = run_with_messages(Kind::Aquarium, &Load::default(), 30, 0.1, &[25]);
    assert_eq!(envelopes(&drawn(&a, band(), &pal)).len(), 1);
}

/// The stars: more the more processes run; faster the more the network
/// carries, streaking at a warp; a message a comet across.
#[test]
fn the_stars_follow_the_processes_and_the_network() {
    let pal = Palette::default();
    let lit = |load: Load| cells(&drawn(&run(Kind::Starfield, &load, 40, 0.05), full(), &pal), |c| c.text() != " ");
    let (few, many) =
        (lit(Load { processes: Some(20), ..Load::default() }), lit(Load { processes: Some(1000), ..Load::default() }));
    assert!(many > few * 2, "{few} {many}");
    let net = |rate: f64| run(Kind::Starfield, &Load { net: Some((rate, 0.0)), ..Load::default() }, 60, 0.05);
    assert!(net(1e9).stars.travel > net(0.0).stars.travel * 5.0);
    let streaks = |a: &Anim| cells(&drawn(a, full(), &pal), |c| ["-", "|", "/", "\\"].contains(&c.text()));
    assert!(streaks(&net(0.0)) == 0 && streaks(&net(1e9)) > 5, "{}", text(&drawn(&net(1e9), full(), &pal)));
    // 9 steps (0.45 s) on: 13 columns across, its tail behind it.
    let a = run_with_messages(Kind::Starfield, &Load::default(), 30, 0.05, &[20]);
    let g = drawn(&a, full(), &pal);
    let e = envelopes(&g);
    assert_eq!(e.len(), 1, "{}", text(&g));
    assert_eq!(e[0].0, 13);
    assert_eq!(g.get(12, e[0].1).text(), "=");
}

/// The game of life keeps Conway's rules; more generations the busier the
/// cores; more alive the more memory is used; a message's glider by the
/// middle, an envelope on it a while.
#[test]
fn the_game_of_life_follows_the_cores_and_memory() {
    // A blinker turns; a block stands, a year older each generation.
    let mut life = life::Life::default();
    life.cells = vec![0; life::GW * life::GH];
    let at = |x: usize, y: usize| y * life::GW + x;
    for x in 10..13 {
        life.cells[at(x, 10)] = 1;
    }
    for (x, y) in [(30, 30), (31, 30), (30, 31), (31, 31)] {
        life.cells[at(x, y)] = 1;
    }
    life.generation();
    let alive: Vec<usize> = (0..life.cells.len()).filter(|&i| life.cells[i] > 0).collect();
    let mut want = vec![at(11, 9), at(11, 10), at(11, 11), at(30, 30), at(31, 30), at(30, 31), at(31, 31)];
    want.sort();
    assert_eq!(alive, want);
    assert_eq!(life.cells[at(30, 30)], 2);
    assert_eq!(life.cells[at(11, 10)], 2, "the blinker's middle lives on");
    // 5 generations a second idle, 15 busy: 10 s.
    let gens = |cpu: f32| run(Kind::Life, &Load { cpu, ..Load::default() }, 100, 0.1).life.gens;
    assert_eq!((gens(0.0), gens(1.0)), (50, 150));
    let alive = |memory: f32| {
        let load = Load { cpu: 1.0, memory: Some(memory), switches: Some(1e6), ..Load::default() };
        run(Kind::Life, &load, 100, 0.1).life.cells.iter().filter(|&&c| c > 0).count()
    };
    assert!(alive(1.0) > alive(0.0) * 3 / 2, "{} {}", alive(0.0), alive(1.0));
    let pal = Palette::default();
    let a = run_with_messages(Kind::Life, &Load::default(), 30, 0.1, &[25]);
    assert_eq!(a.life.gliders.len(), 1);
    assert_eq!(envelopes(&drawn(&a, band(), &pal)).len(), 1, "{}", text(&drawn(&a, band(), &pal)));
    let later = run_with_messages(Kind::Life, &Load::default(), 150, 0.1, &[25]);
    assert!(later.life.gliders.is_empty() && envelopes(&drawn(&later, band(), &pal)).is_empty());
    let g = drawn(&run(Kind::Life, &Load::default(), 10, 0.1), band(), &pal);
    assert!(cells(&g, |c| ["▀", "▄", "█"].contains(&c.text())) > 20, "{}", text(&g));
}

/// The torus: turning faster the busier the cores; thicker the more memory
/// is used; a message turns it once more, its envelope in the hole a
/// while.
#[test]
fn the_donut_turns_with_the_cores() {
    let pal = Palette::default();
    let turned = |cpu: f32| run(Kind::Donut, &Load { cpu, ..Load::default() }, 20, 0.05).donut.a;
    assert!((turned(0.0) - 0.5).abs() < 1e-6 && (turned(1.0) - 3.0).abs() < 1e-6, "{} {}", turned(0.0), turned(1.0));
    let lit = |memory: f32| {
        cells(
            &drawn(&run(Kind::Donut, &Load { memory: Some(memory), ..Load::default() }, 20, 0.05), full(), &pal),
            |c| c.text() != " ",
        )
    };
    assert!(lit(1.0) > lit(0.0) * 6 / 5, "{} {}", lit(0.0), lit(1.0));
    let a = run_with_messages(Kind::Donut, &Load::default(), 21, 0.05, &[20]);
    assert!(a.donut.owed > 5.0, "a turn owed: {}", a.donut.owed);
    let e = envelopes(&drawn(&a, full(), &pal));
    assert_eq!(e, vec![(32, 7)]);
    let later = run_with_messages(Kind::Donut, &Load::default(), 60, 0.05, &[20]);
    assert!(later.donut.owed == 0.0 && envelopes(&drawn(&later, full(), &pal)).is_empty());
    let g = drawn(&run(Kind::Donut, &Load { temp: Some(90.0), ..Load::default() }, 20, 0.05), full(), &pal);
    assert!(cells(&g, |c| c.style.fg == Color::Idx(RED)) > 10, "{}", text(&g));
}

/// The weather: clouds with the cores, rain with the disks, slanting with
/// the network's wind, lightning with the interrupts, water as deep as the
/// memory in use; a message falls into it.
#[test]
fn the_weather_follows_the_machine() {
    let pal = Palette::default();
    let g = |load: Load| drawn(&run(Kind::Weather, &load, 40, 0.066), full(), &pal);
    let count = |load: Load, marks: &[&str]| cells(&g(load), |c| marks.contains(&c.text()));
    let clouds = ["░", "▒", "▓"];
    assert!(count(Load { cpu: 1.0, ..Load::default() }, &clouds) > count(Load::default(), &clouds) * 2);
    let rain = ["|", "\\"];
    let disk = |rate: f64| Load { disk: Some((rate, 0.0)), ..Load::default() };
    assert!(count(disk(1e9), &rain) > count(disk(0.0), &rain) * 4, "{}", text(&g(disk(1e9))));
    let windy = Load { net: Some((1e9, 0.0)), ..disk(1e9) };
    assert!(count(windy.clone(), &["\\"]) > 10 && count(disk(1e9), &["\\"]) == 0, "{}", text(&g(windy)));
    let water = |memory: f32| count(Load { memory: Some(memory), ..Load::default() }, &["~", "-"]);
    assert_eq!(water(0.0), 64, "a row of water");
    assert_eq!(water(1.0), 64 * 5, "a row and a third of the rows more");
    // Lightning: in some quarter second of 10 with the interrupts, never
    // without.
    let bolts = |interrupts: f64| {
        let load = Load { interrupts: Some(interrupts), ..Load::default() };
        let mut a = Anim::new(Kind::Weather, 7);
        (0..40)
            .filter(|_| {
                a.step(0.25, Input { load: load.clone(), ..Input::default() });
                cells(&drawn(&a, full(), &pal), |c| c.style.fg == Color::Idx(BRIGHT_YELLOW)) > 0
            })
            .count()
    };
    assert!(bolts(1e3) == 0 && bolts(1e6) > 3, "{} {}", bolts(1e3), bolts(1e6));
    // From the clouds (row 3 of 14), 12 rows a second: 0.5 s on, row 9.
    let a = run_with_messages(Kind::Weather, &Load::default(), 31, 0.05, &[20]);
    let e = envelopes(&drawn(&a, full(), &pal));
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].1, 3 + 6);
}
