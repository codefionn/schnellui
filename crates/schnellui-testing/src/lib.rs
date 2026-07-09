//! # schnellui-testing
//!
//! The headless test/agent harness (SOUL §7): a scenario abstraction, PNG golden
//! **bless + perceptual diff** (SOUL §7.4 — text/AA make exact byte-match brittle,
//! so we compare with tolerance), and **a11y-tree queries** used as the primary
//! correctness oracle (SOUL §7.5). The a11y assertions and the perceptual diff are
//! fully implemented; scenario *bodies* are supplied by the example crates.

use std::path::{Path, PathBuf};

use schnellui::App;
use schnellui_a11y::{A11yNodeDump, A11yTreeDump};

/// A named scenario: a function that constructs an [`App`] already in — or driven
/// into — its target state (SOUL §7.5 construct/drive). Examples register these in
/// a `clap::ValueEnum` + `strum::EnumIter` table (SOUL §7.1).
pub struct Scenario {
    /// stable scenario name (matches `--scenario`, `--list` output, golden path).
    pub name: &'static str,
    /// builds the app in its target state (SOUL §7.5).
    pub build: fn(u32, u32) -> App,
}

impl Scenario {
    /// Constructs the scenario's app at the given viewport.
    pub fn run(&self, width: u32, height: u32) -> App {
        (self.build)(width, height)
    }
}

/// Snapshot config, sourced from the environment (SOUL §7.4). `SCHNELLUI_BLESS=1`
/// re-blesses goldens; the perceptual `tolerance` guards against AA flake.
#[derive(Clone, Debug)]
pub struct SnapshotConfig {
    pub bless: bool,
    /// max acceptable mean per-channel difference in [0,1] (SOUL §7.4).
    pub tolerance: f64,
    /// directory holding `snapshots/<name>.png` goldens (SOUL §7.4).
    pub snapshot_dir: PathBuf,
}

impl SnapshotConfig {
    /// Reads `SCHNELLUI_BLESS` and defaults the snapshot dir/tolerance (SOUL §7.4).
    pub fn from_env(snapshot_dir: impl Into<PathBuf>) -> SnapshotConfig {
        let bless = std::env::var("SCHNELLUI_BLESS")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        SnapshotConfig {
            bless,
            tolerance: 0.01,
            snapshot_dir: snapshot_dir.into(),
        }
    }

    /// The golden path for a scenario.
    pub fn golden_path(&self, name: &str) -> PathBuf {
        self.snapshot_dir.join(format!("{name}.png"))
    }

    /// The diff-artifact path for a scenario (SOUL §7.4 — `*.diff.png`, git-ignored).
    pub fn diff_path(&self, name: &str) -> PathBuf {
        self.snapshot_dir.join(format!("{name}.diff.png"))
    }
}

/// The outcome of a perceptual comparison (SOUL §7.4).
#[derive(Clone, Debug)]
pub struct DiffResult {
    /// mean per-channel absolute difference, normalized to [0,1].
    pub score: f64,
    /// `true` if `score <= tolerance`.
    pub matches: bool,
    /// per-pixel diff image (RGBA8), present only on mismatch.
    pub diff_image: Option<Vec<u8>>,
}

/// Compares two tightly-packed RGBA8 buffers with a perceptual tolerance
/// (SOUL §7.4). Produces a visual diff image on mismatch (highlighting changed
/// pixels in red) so an agent can look at it.
pub fn compare_rgba(a: &[u8], b: &[u8], tolerance: f64) -> DiffResult {
    assert_eq!(a.len(), b.len(), "image buffers differ in size");
    let mut acc: u64 = 0;
    let mut diff = vec![0u8; a.len()];
    let mut any = false;
    for i in (0..a.len()).step_by(4) {
        let mut px_diff = 0u32;
        for c in 0..4 {
            let d = (a[i + c] as i32 - b[i + c] as i32).unsigned_abs();
            acc += d as u64;
            px_diff += d;
        }
        if px_diff > 0 {
            any = true;
            // highlight changed pixels opaque red
            diff[i] = 255;
            diff[i + 1] = 0;
            diff[i + 2] = 0;
            diff[i + 3] = 255;
        } else {
            // faded original for context
            diff[i] = a[i] / 4;
            diff[i + 1] = a[i + 1] / 4;
            diff[i + 2] = a[i + 2] / 4;
            diff[i + 3] = 255;
        }
    }
    let score = acc as f64 / (a.len() as f64 * 255.0);
    let matches = score <= tolerance;
    DiffResult {
        score,
        matches,
        diff_image: if any && !matches { Some(diff) } else { None },
    }
}

/// Blesses (writes) `rgba` as the golden if `cfg.bless`, else compares against the
/// existing golden with tolerance (SOUL §7.4). On mismatch, writes the diff PNG
/// artifact next to the golden and returns the failing [`DiffResult`].
pub fn bless_or_compare(
    name: &str,
    rgba: &[u8],
    width: u32,
    height: u32,
    cfg: &SnapshotConfig,
) -> std::io::Result<DiffResult> {
    let golden = cfg.golden_path(name);
    if cfg.bless || !golden.exists() {
        std::fs::create_dir_all(&cfg.snapshot_dir)?;
        let png = schnellui_render_wgpu_encode(rgba, width, height);
        std::fs::write(&golden, png)?;
        return Ok(DiffResult {
            score: 0.0,
            matches: true,
            diff_image: None,
        });
    }
    let golden_rgba = load_png_rgba(&golden)?;
    let result = compare_rgba(rgba, &golden_rgba, cfg.tolerance);
    if let Some(diff) = &result.diff_image {
        let png = schnellui_render_wgpu_encode(diff, width, height);
        std::fs::write(cfg.diff_path(name), png)?;
    }
    Ok(result)
}

/// Encodes RGBA8 → PNG (delegates to the backend encoder, SOUL §7.2).
fn schnellui_render_wgpu_encode(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    schnellui::render::encode_png(rgba, width, height)
}

/// Loads a PNG file to a tightly-packed RGBA8 buffer (SOUL §7.4).
fn load_png_rgba(path: &Path) -> std::io::Result<Vec<u8>> {
    let img =
        image::open(path).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(img.to_rgba8().into_raw())
}

// --- a11y-tree oracle (SOUL §7.5) ---

/// Finds the first node with the given role (and, if `name` is `Some`, that name)
/// anywhere in the dumped a11y tree (SOUL §7.5 — locate by semantics, never pixels).
pub fn find_by_role_name<'a>(
    tree: &'a A11yTreeDump,
    role: &str,
    name: Option<&str>,
) -> Option<&'a A11yNodeDump> {
    fn walk<'a>(
        node: &'a A11yNodeDump,
        role: &str,
        name: Option<&str>,
    ) -> Option<&'a A11yNodeDump> {
        if node.role == role
            && name
                .map(|n| node.name.as_deref() == Some(n))
                .unwrap_or(true)
        {
            return Some(node);
        }
        for c in &node.children {
            if let Some(found) = walk(c, role, name) {
                return Some(found);
            }
        }
        None
    }
    tree.root.as_ref().and_then(|r| walk(r, role, name))
}

/// Asserts a node of `role`/`name` exists whose value contains `needle`
/// (SOUL §7.5 `.value_contains`). Returns `Ok` or a human-readable failure.
pub fn assert_value_contains(
    tree: &A11yTreeDump,
    role: &str,
    name: Option<&str>,
    needle: &str,
) -> Result<(), String> {
    let node = find_by_role_name(tree, role, name)
        .ok_or_else(|| format!("no node with role={role} name={name:?}"))?;
    match &node.value {
        Some(v) if v.contains(needle) => Ok(()),
        other => Err(format!(
            "node role={role} value={other:?} does not contain {needle:?}"
        )),
    }
}

/// Asserts a node of `role`/`name` is not disabled (SOUL §7.5 `.is_enabled`).
pub fn assert_enabled(tree: &A11yTreeDump, role: &str, name: Option<&str>) -> Result<(), String> {
    let node = find_by_role_name(tree, role, name)
        .ok_or_else(|| format!("no node with role={role} name={name:?}"))?;
    if node.state.iter().any(|s| s == "disabled") {
        Err(format!("node role={role} name={name:?} is disabled"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump() -> A11yTreeDump {
        A11yTreeDump {
            focus: None,
            root: Some(A11yNodeDump {
                id: 1,
                role: "column".into(),
                children: vec![
                    A11yNodeDump {
                        id: 2,
                        role: "status".into(),
                        value: Some("count: 5".into()),
                        ..Default::default()
                    },
                    A11yNodeDump {
                        id: 3,
                        role: "button".into(),
                        name: Some("increment".into()),
                        actions: vec!["click".into()],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
        }
    }

    #[test]
    fn identical_images_match() {
        let a = vec![10u8, 20, 30, 255, 40, 50, 60, 255];
        let r = compare_rgba(&a, &a, 0.0);
        assert!(r.matches);
        assert_eq!(r.score, 0.0);
        assert!(r.diff_image.is_none());
    }

    #[test]
    fn differing_images_flag_and_diff() {
        let a = vec![0u8, 0, 0, 255];
        let b = vec![255u8, 255, 255, 255];
        let r = compare_rgba(&a, &b, 0.01);
        assert!(!r.matches);
        assert!(r.score > 0.5);
        assert!(r.diff_image.is_some());
    }

    #[test]
    fn find_by_role_and_name() {
        let t = dump();
        assert!(find_by_role_name(&t, "button", Some("increment")).is_some());
        assert!(find_by_role_name(&t, "button", Some("decrement")).is_none());
        assert!(find_by_role_name(&t, "status", None).is_some());
    }

    #[test]
    fn value_contains_and_enabled() {
        let t = dump();
        assert!(assert_value_contains(&t, "status", None, "5").is_ok());
        assert!(assert_value_contains(&t, "status", None, "9").is_err());
        assert!(assert_enabled(&t, "button", Some("increment")).is_ok());
    }

    #[test]
    fn snapshot_config_from_env_defaults() {
        let cfg = SnapshotConfig::from_env("snapshots");
        assert_eq!(
            cfg.golden_path("counter"),
            PathBuf::from("snapshots/counter.png")
        );
        assert_eq!(
            cfg.diff_path("counter"),
            PathBuf::from("snapshots/counter.diff.png")
        );
    }
}
