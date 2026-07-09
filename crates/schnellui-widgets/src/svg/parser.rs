use super::*;

pub fn parse_svg(markup: &str) -> Result<SvgDoc, String> {
    let mut doc = SvgDoc {
        min_x: 0.0,
        min_y: 0.0,
        width: 0.0,
        height: 0.0,
        gradients: Vec::new(),
        shapes: Vec::new(),
    };
    let mut saw_svg = false;
    // shapes carry unresolved paints until the whole document is scanned
    let mut pending: Vec<(SvgShape, Option<PaintRef>, Option<PaintRef>)> = Vec::new();
    let mut gradient_ids: Vec<String> = Vec::new();
    // an open gradient collecting `<stop>`s
    let mut open_gradient: Option<Gradient> = None;
    let mut open_gradient_id = String::new();
    let mut stack: Vec<Inherited> = vec![Inherited::root()];

    let mut rest = markup;
    while let Some(lt) = rest.find('<') {
        rest = &rest[lt + 1..];
        if let Some(r) = rest.strip_prefix("!--") {
            rest = match r.find("-->") {
                Some(i) => &r[i + 3..],
                None => "",
            };
            continue;
        }
        if rest.starts_with('!') || rest.starts_with('?') {
            rest = match rest.find('>') {
                Some(i) => &rest[i + 1..],
                None => "",
            };
            continue;
        }
        // closing tags pop the matching scopes
        if let Some(r) = rest.strip_prefix('/') {
            let Some(gt) = r.find('>') else { break };
            let name = r[..gt].trim();
            rest = &r[gt + 1..];
            match name {
                "g" | "svg" if stack.len() > 1 => {
                    stack.pop();
                }
                "linearGradient" | "radialGradient" => {
                    if let Some(g) = open_gradient.take() {
                        gradient_ids.push(std::mem::take(&mut open_gradient_id));
                        doc.gradients.push(g);
                    }
                }
                _ => {}
            }
            continue;
        }
        let Some(gt) = rest.find('>') else { break };
        let raw_tag = &rest[..gt];
        let self_closing = raw_tag.trim_end().ends_with('/');
        let tag = raw_tag.trim_end_matches('/').trim();
        rest = &rest[gt + 1..];

        let (name, attr_str) = match tag.find(|c: char| c.is_whitespace()) {
            Some(i) => (&tag[..i], &tag[i..]),
            None => (tag, ""),
        };
        let attrs = parse_attrs(attr_str);
        let get = |k: &str| attrs.iter().find(|(n, _)| *n == k).map(|(_, v)| *v);
        let num = |k: &str, default: f32| {
            get(k)
                .and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok())
                .unwrap_or(default)
        };

        // --- gradient definitions (collected wherever they appear) ---
        match name {
            "linearGradient" => {
                let g = Gradient {
                    kind: GradientKind::Linear {
                        x1: num("x1", 0.0),
                        y1: num("y1", 0.0),
                        x2: num("x2", 1.0),
                        y2: num("y2", 0.0),
                    },
                    object_units: get("gradientUnits") != Some("userSpaceOnUse"),
                    stops: Vec::new(),
                };
                let id = get("id").unwrap_or("").to_string();
                if self_closing {
                    gradient_ids.push(id);
                    doc.gradients.push(g);
                } else {
                    open_gradient = Some(g);
                    open_gradient_id = id;
                }
                continue;
            }
            "radialGradient" => {
                let g = Gradient {
                    kind: GradientKind::Radial {
                        cx: num("cx", 0.5),
                        cy: num("cy", 0.5),
                        r: num("r", 0.5),
                    },
                    object_units: get("gradientUnits") != Some("userSpaceOnUse"),
                    stops: Vec::new(),
                };
                let id = get("id").unwrap_or("").to_string();
                if self_closing {
                    gradient_ids.push(id);
                    doc.gradients.push(g);
                } else {
                    open_gradient = Some(g);
                    open_gradient_id = id;
                }
                continue;
            }
            "stop" => {
                if let Some(g) = &mut open_gradient {
                    let offset = {
                        let raw = get("offset").unwrap_or("0").trim();
                        match raw.strip_suffix('%') {
                            Some(p) => p.parse::<f32>().unwrap_or(0.0) / 100.0,
                            None => raw.parse::<f32>().unwrap_or(0.0),
                        }
                    }
                    .clamp(0.0, 1.0);
                    let mut color = get("stop-color")
                        .and_then(parse_color)
                        .flatten()
                        .unwrap_or(Color::BLACK);
                    if let Some(op) = get("stop-opacity").and_then(|v| v.parse::<f32>().ok()) {
                        color.a = (color.a as f32 * op.clamp(0.0, 1.0)).round() as u8;
                    }
                    // keep offsets monotonic (SVG clamps to the previous stop)
                    let floor = g.stops.last().map(|(o, _)| *o).unwrap_or(0.0);
                    g.stops.push((offset.max(floor), color));
                }
                continue;
            }
            _ => {}
        }

        // --- inheritable presentation for this element ---
        let parent = stack.last().expect("root scope").clone();
        let own_fill: Option<Option<PaintRef>> = get("fill").and_then(parse_paint);
        let own_stroke: Option<Option<PaintRef>> = get("stroke").and_then(parse_paint);
        let own_sw =
            get("stroke-width").and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok());
        let own_rule = get("fill-rule").and_then(|v| match v {
            "evenodd" => Some(FillRule::EvenOdd),
            "nonzero" => Some(FillRule::NonZero),
            _ => None,
        });
        let own_font =
            get("font-size").and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok());
        let own_anchor = get("text-anchor").and_then(|v| match v {
            "start" => Some(TextAnchor::Start),
            "middle" => Some(TextAnchor::Middle),
            "end" => Some(TextAnchor::End),
            _ => None,
        });
        let own_opacity = get("opacity")
            .and_then(|v| v.parse::<f32>().ok())
            .map(|v| v.clamp(0.0, 1.0))
            .unwrap_or(1.0);
        let transform = parent.transform.then(
            &get("transform")
                .map(parse_transform)
                .unwrap_or(Transform2::IDENTITY),
        );
        let scope = Inherited {
            fill: own_fill.clone().or(parent.fill.clone()),
            stroke: own_stroke.clone().or(parent.stroke.clone()),
            stroke_width: own_sw.or(parent.stroke_width),
            fill_rule: own_rule.or(parent.fill_rule),
            font_size: own_font.or(parent.font_size),
            text_anchor: own_anchor.or(parent.text_anchor),
            opacity: parent.opacity * own_opacity,
            transform,
        };

        // --- structural elements ---
        match name {
            "svg" => {
                saw_svg = true;
                if let Some(vb) = get("viewBox").or_else(|| get("viewbox")) {
                    let v: Vec<f32> = vb
                        .split(|c: char| c.is_whitespace() || c == ',')
                        .filter(|s| !s.is_empty())
                        .filter_map(|s| s.parse().ok())
                        .collect();
                    if v.len() == 4 {
                        doc.min_x = v[0];
                        doc.min_y = v[1];
                        doc.width = v[2];
                        doc.height = v[3];
                    }
                }
                if doc.width <= 0.0 || doc.height <= 0.0 {
                    doc.width = num("width", 0.0);
                    doc.height = num("height", 0.0);
                }
                if !self_closing {
                    stack.push(scope);
                }
                continue;
            }
            "g" | "defs" => {
                if !self_closing {
                    stack.push(scope);
                }
                continue;
            }
            _ => {}
        }

        // --- geometry elements: resolve presentation, flatten, queue ---
        let fill_ref: Option<PaintRef> = scope
            .fill
            .clone()
            .unwrap_or(Some(PaintRef::Solid(Color::BLACK))); // SVG initial fill
        let stroke_ref: Option<PaintRef> = scope.stroke.clone().flatten();
        let stroke_width = scope.stroke_width.unwrap_or(1.0) * transform.scale_factor();
        let fill_rule = scope.fill_rule.unwrap_or_default();
        let opacity = scope.opacity;
        let mut queue = |kind: SvgShapeKind, fill: Option<PaintRef>, stroke: Option<PaintRef>| {
            pending.push((
                SvgShape {
                    kind,
                    fill: None,
                    stroke: None,
                    stroke_width,
                    fill_rule,
                    opacity,
                    transform,
                },
                fill,
                stroke,
            ));
        };

        match name {
            "rect" => {
                let (w, h) = (num("width", 0.0), num("height", 0.0));
                if w > 0.0 && h > 0.0 {
                    let rx = num("rx", num("ry", 0.0)).min(w * 0.5);
                    let ry = num("ry", num("rx", 0.0)).min(h * 0.5);
                    let c = flatten_round_rect(num("x", 0.0), num("y", 0.0), w, h, rx, ry);
                    queue(
                        SvgShapeKind::Path {
                            contours: vec![baked(c, &transform)],
                        },
                        fill_ref,
                        stroke_ref,
                    );
                }
            }
            "circle" => {
                let r = num("r", 0.0);
                if r > 0.0 {
                    let c = flatten_ellipse(num("cx", 0.0), num("cy", 0.0), r, r);
                    queue(
                        SvgShapeKind::Path {
                            contours: vec![baked(c, &transform)],
                        },
                        fill_ref,
                        stroke_ref,
                    );
                }
            }
            "ellipse" => {
                let (rx, ry) = (num("rx", 0.0), num("ry", 0.0));
                if rx > 0.0 && ry > 0.0 {
                    let c = flatten_ellipse(num("cx", 0.0), num("cy", 0.0), rx, ry);
                    queue(
                        SvgShapeKind::Path {
                            contours: vec![baked(c, &transform)],
                        },
                        fill_ref,
                        stroke_ref,
                    );
                }
            }
            "line" => {
                let c = Contour {
                    pts: vec![
                        (num("x1", 0.0), num("y1", 0.0)),
                        (num("x2", 0.0), num("y2", 0.0)),
                    ],
                    closed: false,
                };
                // a bare segment has no interior — fill never applies
                queue(
                    SvgShapeKind::Path {
                        contours: vec![baked(c, &transform)],
                    },
                    None,
                    stroke_ref,
                );
            }
            "polyline" | "polygon" => {
                let pts = parse_points(get("points").unwrap_or(""));
                if pts.len() >= 2 {
                    let c = Contour {
                        pts,
                        closed: name == "polygon",
                    };
                    queue(
                        SvgShapeKind::Path {
                            contours: vec![baked(c, &transform)],
                        },
                        fill_ref,
                        stroke_ref,
                    );
                }
            }
            "path" => {
                let contours: Vec<Contour> = flatten_path(get("d").unwrap_or(""))
                    .into_iter()
                    .map(|c| baked(c, &transform))
                    .filter(|c| c.pts.len() >= 2)
                    .collect();
                if !contours.is_empty() {
                    queue(SvgShapeKind::Path { contours }, fill_ref, stroke_ref);
                }
            }
            "text" => {
                // content sits between <text …> and </text>
                let content = if self_closing {
                    String::new()
                } else {
                    match rest.find("</text") {
                        Some(end) => {
                            let c = rest[..end].trim().to_string();
                            rest = &rest[end..];
                            c
                        }
                        None => String::new(),
                    }
                };
                if !content.is_empty() {
                    let (x, y) = transform.apply((num("x", 0.0), num("y", 0.0)));
                    queue(
                        SvgShapeKind::Text {
                            x,
                            y,
                            size: scope.font_size.unwrap_or(16.0) * transform.scale_factor(),
                            anchor: scope.text_anchor.unwrap_or_default(),
                            content,
                        },
                        fill_ref,
                        stroke_ref,
                    );
                }
            }
            _ => {} // title / desc / unknown: skipped
        }
    }

    if !saw_svg || doc.width <= 0.0 || doc.height <= 0.0 {
        return Err("svg subset: no <svg> with a usable viewBox/width/height".to_string());
    }

    // resolve paint references now that every gradient definition is known
    let resolve = |r: Option<PaintRef>| -> Option<Paint> {
        match r? {
            PaintRef::Solid(c) => Some(Paint::Solid(c)),
            PaintRef::Url(id) => Some(
                gradient_ids
                    .iter()
                    .position(|g| *g == id)
                    .map(Paint::Gradient)
                    // a dangling url() paints black rather than vanishing
                    .unwrap_or(Paint::Solid(Color::BLACK)),
            ),
        }
    };
    for (mut shape, fill, stroke) in pending {
        shape.fill = resolve(fill);
        shape.stroke = resolve(stroke);
        doc.shapes.push(shape);
    }
    Ok(doc)
}

/// Applies a transform to a contour (bakes the CTM into the points).
fn baked(c: Contour, t: &Transform2) -> Contour {
    Contour {
        pts: c.pts.iter().map(|p| t.apply(*p)).collect(),
        closed: c.closed,
    }
}

/// Parses a paint value: `Some(None)` = `none`, `Some(Some(ref))` = paintable.
/// `None` = unparseable (treated as not-set so inheritance applies).
fn parse_paint(s: &str) -> Option<Option<PaintRef>> {
    let s = s.trim();
    if let Some(url) = s.strip_prefix("url(") {
        let id = url
            .trim_end_matches(')')
            .trim()
            .trim_matches(|c| c == '\'' || c == '"')
            .trim_start_matches('#');
        return Some(Some(PaintRef::Url(id.to_string())));
    }
    parse_color(s).map(|c| c.map(PaintRef::Solid))
}

/// Parses `name="value"` (or single-quoted) attribute pairs out of a tag body.
fn parse_attrs(s: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let name_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' {
            i += 1;
        }
        if i == name_start {
            break;
        }
        let name = &s[name_start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || (bytes[i] != b'"' && bytes[i] != b'\'') {
            continue;
        }
        let quote = bytes[i];
        i += 1;
        let val_start = i;
        while i < bytes.len() && bytes[i] != quote {
            i += 1;
        }
        if i > bytes.len() {
            break;
        }
        out.push((name, &s[val_start..i]));
        i += 1;
    }
    out
}

/// Parses a `points="x1,y1 x2,y2 …"` list.
fn parse_points(s: &str) -> Vec<(f32, f32)> {
    let nums: Vec<f32> = s
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .filter_map(|t| t.parse().ok())
        .collect();
    nums.chunks_exact(2).map(|p| (p[0], p[1])).collect()
}

/// Parses a subset color: `#rgb`, `#rrggbb`, `rgb(r,g,b)`, a small named set,
/// or `none`. `None` = unparseable, `Some(None)` = `none`.
#[allow(clippy::option_option)]
fn parse_color(s: &str) -> Option<Option<Color>> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("none") {
        return Some(None);
    }
    if let Some(hex) = s.strip_prefix('#') {
        let v = u32::from_str_radix(hex, 16).ok()?;
        return match hex.len() {
            3 => {
                let r = ((v >> 8) & 0xf) as u8;
                let g = ((v >> 4) & 0xf) as u8;
                let b = (v & 0xf) as u8;
                Some(Some(Color::rgb(r << 4 | r, g << 4 | g, b << 4 | b)))
            }
            6 => Some(Some(Color::rgb(
                ((v >> 16) & 0xff) as u8,
                ((v >> 8) & 0xff) as u8,
                (v & 0xff) as u8,
            ))),
            _ => None,
        };
    }
    if let Some(body) = s.strip_prefix("rgba(").or_else(|| s.strip_prefix("rgb(")) {
        let vals: Vec<f32> = body
            .trim_end_matches(')')
            .split(',')
            .filter_map(|t| t.trim().parse().ok())
            .collect();
        if vals.len() >= 3 {
            let a = vals.get(3).map(|a| (a * 255.0) as u8).unwrap_or(255);
            return Some(Some(Color::rgba(
                vals[0] as u8,
                vals[1] as u8,
                vals[2] as u8,
                a,
            )));
        }
        return None;
    }
    let named = match s.to_ascii_lowercase().as_str() {
        "black" | "currentcolor" => Color::BLACK,
        "white" => Color::WHITE,
        "red" => Color::rgb(0xff, 0x00, 0x00),
        "green" => Color::rgb(0x00, 0x80, 0x00),
        "lime" => Color::rgb(0x00, 0xff, 0x00),
        "blue" => Color::rgb(0x00, 0x00, 0xff),
        "yellow" => Color::rgb(0xff, 0xff, 0x00),
        "orange" => Color::rgb(0xff, 0xa5, 0x00),
        "purple" => Color::rgb(0x80, 0x00, 0x80),
        "gray" | "grey" => Color::rgb(0x80, 0x80, 0x80),
        _ => return None,
    };
    Some(Some(named))
}

// ---------------------------------------------------------------------------
// geometry flattening (curves/arcs → line segments, deterministic — SOUL §7.3)
// ---------------------------------------------------------------------------

/// Segment count for a curve whose control net spans roughly `len` user units:
/// generous and fixed (icons raster at ≤ a few px per unit; SS hides the rest).
fn segments_for(len: f32) -> usize {
    ((len * 2.0) as usize).clamp(8, 96)
}

/// Flattens an axis-aligned (possibly rounded) rect to one closed contour.
fn flatten_round_rect(x: f32, y: f32, w: f32, h: f32, rx: f32, ry: f32) -> Contour {
    if rx <= 0.0 || ry <= 0.0 {
        return Contour {
            pts: vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)],
            closed: true,
        };
    }
    // four corner arcs, each flattened
    let n = segments_for((rx + ry) * 0.5 * std::f32::consts::FRAC_PI_2).max(4);
    let mut pts = Vec::new();
    let corners = [
        (x + w - rx, y + ry, -std::f32::consts::FRAC_PI_2, 0.0f32), // top-right
        (x + w - rx, y + h - ry, 0.0, std::f32::consts::FRAC_PI_2), // bottom-right
        (
            x + rx,
            y + h - ry,
            std::f32::consts::FRAC_PI_2,
            std::f32::consts::PI,
        ), // bottom-left
        (
            x + rx,
            y + ry,
            std::f32::consts::PI,
            1.5 * std::f32::consts::PI,
        ), // top-left
    ];
    for (cx, cy, a0, a1) in corners {
        for i in 0..=n {
            let t = a0 + (a1 - a0) * (i as f32 / n as f32);
            pts.push((cx + rx * t.cos(), cy + ry * t.sin()));
        }
    }
    Contour { pts, closed: true }
}

/// Flattens an ellipse to one closed contour.
fn flatten_ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Contour {
    let n = segments_for(std::f32::consts::PI * (rx + ry)).clamp(24, 128);
    let pts = (0..n)
        .map(|i| {
            let t = std::f32::consts::TAU * (i as f32) / (n as f32);
            (cx + rx * t.cos(), cy + ry * t.sin())
        })
        .collect();
    Contour { pts, closed: true }
}

/// Flattens the `d` path-data subset into contours: `M L H V C S Q T A Z` and
/// relative forms; curves/arcs subdivide, coordinates after `M`/`L` continue as
/// implicit linetos, per spec.
fn flatten_path(d: &str) -> Vec<Contour> {
    enum Tok {
        Cmd(char),
        Num(f32),
    }
    let mut toks = Vec::new();
    let mut num = String::new();
    let flush = |num: &mut String, toks: &mut Vec<Tok>| {
        if !num.is_empty() {
            if let Ok(v) = num.parse::<f32>() {
                toks.push(Tok::Num(v));
            }
            num.clear();
        }
    };
    for c in d.chars() {
        match c {
            // SVG numbers may use scientific notation. `e`/`E` are not path
            // commands, so keep them with a number when they occur after digits.
            'e' | 'E' if !num.is_empty() && !num.contains(['e', 'E']) => num.push(c),
            'a'..='z' | 'A'..='Z' => {
                flush(&mut num, &mut toks);
                toks.push(Tok::Cmd(c));
            }
            '0'..='9' => num.push(c),
            // A decimal point after an existing decimal starts the next SVG
            // number even without whitespace: `3.41.81` means `3.41, .81`.
            // Material Design paths use this compact form extensively.
            '.' => {
                let mantissa = num
                    .split_once(['e', 'E'])
                    .map(|(head, _)| head)
                    .unwrap_or(&num);
                if mantissa.contains('.') {
                    flush(&mut num, &mut toks);
                }
                num.push(c);
            }
            '-' | '+' => {
                if !num.is_empty() && !num.ends_with(['e', 'E']) {
                    flush(&mut num, &mut toks);
                }
                num.push(c);
            }
            _ => flush(&mut num, &mut toks),
        }
    }
    flush(&mut num, &mut toks);

    let mut out: Vec<Contour> = Vec::new();
    let mut cur: Vec<(f32, f32)> = Vec::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let (mut start_x, mut start_y) = (0.0f32, 0.0f32);
    // previous control point, for S/T reflection
    let mut prev_cubic_ctrl: Option<(f32, f32)> = None;
    let mut prev_quad_ctrl: Option<(f32, f32)> = None;
    let mut cmd = 'M';
    let mut i = 0;
    let take = |toks: &[Tok], i: &mut usize| -> Option<f32> {
        match toks.get(*i) {
            Some(Tok::Num(v)) => {
                *i += 1;
                Some(*v)
            }
            _ => None,
        }
    };
    macro_rules! take2 {
        () => {
            match (take(&toks, &mut i), take(&toks, &mut i)) {
                (Some(a), Some(b)) => (a, b),
                _ => break,
            }
        };
    }
    while i < toks.len() {
        if let Tok::Cmd(c) = toks[i] {
            cmd = c;
            i += 1;
            if cmd == 'Z' || cmd == 'z' {
                if cur.len() >= 2 {
                    out.push(Contour {
                        pts: std::mem::take(&mut cur),
                        closed: true,
                    });
                } else {
                    cur.clear();
                }
                x = start_x;
                y = start_y;
                prev_cubic_ctrl = None;
                prev_quad_ctrl = None;
                continue;
            }
        }
        let rel = cmd.is_ascii_lowercase();
        match cmd.to_ascii_uppercase() {
            'M' | 'L' => {
                let (a, b) = take2!();
                let (nx, ny) = if rel { (x + a, y + b) } else { (a, b) };
                let moving = cmd == 'M' || cmd == 'm';
                if moving {
                    if cur.len() >= 2 {
                        out.push(Contour {
                            pts: std::mem::take(&mut cur),
                            closed: false,
                        });
                    } else {
                        cur.clear();
                    }
                    start_x = nx;
                    start_y = ny;
                }
                cur.push((nx, ny));
                x = nx;
                y = ny;
                prev_cubic_ctrl = None;
                prev_quad_ctrl = None;
                if moving {
                    cmd = if rel { 'l' } else { 'L' };
                }
            }
            'H' => {
                let Some(a) = take(&toks, &mut i) else { break };
                x = if rel { x + a } else { a };
                cur.push((x, y));
                prev_cubic_ctrl = None;
                prev_quad_ctrl = None;
            }
            'V' => {
                let Some(a) = take(&toks, &mut i) else { break };
                y = if rel { y + a } else { a };
                cur.push((x, y));
                prev_cubic_ctrl = None;
                prev_quad_ctrl = None;
            }
            'C' | 'S' => {
                let (c1, c2, end);
                if cmd.eq_ignore_ascii_case(&'C') {
                    let (a, b) = take2!();
                    let (c, d) = take2!();
                    let (e, f) = take2!();
                    c1 = if rel { (x + a, y + b) } else { (a, b) };
                    c2 = if rel { (x + c, y + d) } else { (c, d) };
                    end = if rel { (x + e, y + f) } else { (e, f) };
                } else {
                    // S: first control reflects the previous cubic control
                    let (c, d) = take2!();
                    let (e, f) = take2!();
                    c1 = match prev_cubic_ctrl {
                        Some((px, py)) => (2.0 * x - px, 2.0 * y - py),
                        None => (x, y),
                    };
                    c2 = if rel { (x + c, y + d) } else { (c, d) };
                    end = if rel { (x + e, y + f) } else { (e, f) };
                }
                let net = dist((x, y), c1) + dist(c1, c2) + dist(c2, end);
                let n = segments_for(net);
                for k in 1..=n {
                    let t = k as f32 / n as f32;
                    cur.push(cubic_at((x, y), c1, c2, end, t));
                }
                prev_cubic_ctrl = Some(c2);
                prev_quad_ctrl = None;
                x = end.0;
                y = end.1;
            }
            'Q' | 'T' => {
                let (c1, end);
                if cmd.eq_ignore_ascii_case(&'Q') {
                    let (a, b) = take2!();
                    let (e, f) = take2!();
                    c1 = if rel { (x + a, y + b) } else { (a, b) };
                    end = if rel { (x + e, y + f) } else { (e, f) };
                } else {
                    // T: control reflects the previous quadratic control
                    let (e, f) = take2!();
                    c1 = match prev_quad_ctrl {
                        Some((px, py)) => (2.0 * x - px, 2.0 * y - py),
                        None => (x, y),
                    };
                    end = if rel { (x + e, y + f) } else { (e, f) };
                }
                let net = dist((x, y), c1) + dist(c1, end);
                let n = segments_for(net);
                for k in 1..=n {
                    let t = k as f32 / n as f32;
                    cur.push(quad_at((x, y), c1, end, t));
                }
                prev_quad_ctrl = Some(c1);
                prev_cubic_ctrl = None;
                x = end.0;
                y = end.1;
            }
            'A' => {
                // rx ry x-rot large-arc sweep x y
                let (rx, ry) = take2!();
                let Some(rot) = take(&toks, &mut i) else {
                    break;
                };
                let (laf, sf) = take2!();
                let (e, f) = take2!();
                let end = if rel { (x + e, y + f) } else { (e, f) };
                flatten_arc(
                    &mut cur,
                    (x, y),
                    end,
                    rx.abs(),
                    ry.abs(),
                    rot.to_radians(),
                    laf != 0.0,
                    sf != 0.0,
                );
                prev_cubic_ctrl = None;
                prev_quad_ctrl = None;
                x = end.0;
                y = end.1;
            }
            _ => {
                while matches!(toks.get(i), Some(Tok::Num(_))) {
                    i += 1;
                }
            }
        }
    }
    if cur.len() >= 2 {
        out.push(Contour {
            pts: cur,
            closed: false,
        });
    }
    out
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn cubic_at(p0: (f32, f32), c1: (f32, f32), c2: (f32, f32), p1: (f32, f32), t: f32) -> (f32, f32) {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    (
        a * p0.0 + b * c1.0 + c * c2.0 + d * p1.0,
        a * p0.1 + b * c1.1 + c * c2.1 + d * p1.1,
    )
}

fn quad_at(p0: (f32, f32), c1: (f32, f32), p1: (f32, f32), t: f32) -> (f32, f32) {
    let u = 1.0 - t;
    let (a, b, c) = (u * u, 2.0 * u * t, t * t);
    (
        a * p0.0 + b * c1.0 + c * p1.0,
        a * p0.1 + b * c1.1 + c * p1.1,
    )
}

/// Flattens an SVG elliptical arc (endpoint parameterization → center form, per
/// the SVG implementation notes) into line segments appended to `cur`.
#[allow(clippy::too_many_arguments)]
fn flatten_arc(
    cur: &mut Vec<(f32, f32)>,
    from: (f32, f32),
    to: (f32, f32),
    mut rx: f32,
    mut ry: f32,
    rot: f32,
    large_arc: bool,
    sweep: bool,
) {
    if rx <= 0.0 || ry <= 0.0 || from == to {
        cur.push(to);
        return;
    }
    let (cos_r, sin_r) = (rot.cos(), rot.sin());
    // step 1: midpoint form
    let dx2 = (from.0 - to.0) * 0.5;
    let dy2 = (from.1 - to.1) * 0.5;
    let x1p = cos_r * dx2 + sin_r * dy2;
    let y1p = -sin_r * dx2 + cos_r * dy2;
    // radii scale-up when too small
    let lambda = (x1p / rx).powi(2) + (y1p / ry).powi(2);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    // step 2: center in the primed frame
    let num = (rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p).max(0.0);
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut coef = if den > 0.0 { (num / den).sqrt() } else { 0.0 };
    if large_arc == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1p / ry;
    let cyp = -coef * ry * x1p / rx;
    // step 3: center + angles
    let cx = cos_r * cxp - sin_r * cyp + (from.0 + to.0) * 0.5;
    let cy = sin_r * cxp + cos_r * cyp + (from.1 + to.1) * 0.5;
    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| -> f32 {
        let dot = ux * vx + uy * vy;
        let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let mut a = (dot / len.max(f32::EPSILON)).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 {
            a = -a;
        }
        a
    };
    let theta1 = angle(1.0, 0.0, (x1p - cxp) / rx, (y1p - cyp) / ry);
    let mut dtheta = angle(
        (x1p - cxp) / rx,
        (y1p - cyp) / ry,
        (-x1p - cxp) / rx,
        (-y1p - cyp) / ry,
    );
    if !sweep && dtheta > 0.0 {
        dtheta -= std::f32::consts::TAU;
    }
    if sweep && dtheta < 0.0 {
        dtheta += std::f32::consts::TAU;
    }
    let n = segments_for((rx + ry) * 0.5 * dtheta.abs()).max(8);
    for k in 1..=n {
        let t = theta1 + dtheta * (k as f32 / n as f32);
        let (ct, st) = (t.cos(), t.sin());
        cur.push((
            cx + rx * ct * cos_r - ry * st * sin_r,
            cy + rx * ct * sin_r + ry * st * cos_r,
        ));
    }
    // land exactly on the endpoint
    if let Some(last) = cur.last_mut() {
        *last = to;
    }
}
