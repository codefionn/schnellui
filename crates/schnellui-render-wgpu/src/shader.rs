pub(crate) const SHADER_SRC: &str = r#"
struct Uniforms {
    viewport: vec2<f32>,
    atlas_size: vec2<f32>,
    params: vec4<f32>,   // params.x = logical->physical scale (SOUL §7.1 --scale)
};
@group(0) @binding(0) var<uniform> u: Uniforms;

@group(1) @binding(0) var atlas_tex: texture_2d<f32>;
@group(1) @binding(1) var atlas_samp: sampler;

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + vec3<f32>(0.055)) / vec3<f32>(1.055), vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

fn unit_corner(vid: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    return corners[vid];
}

fn to_ndc(p: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(
        p.x / u.viewport.x * 2.0 - 1.0,
        1.0 - p.y / u.viewport.y * 2.0,
        0.0,
        1.0,
    );
}

struct QuadOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec2<f32>,
    @location(2) half_size: vec2<f32>,
    @location(3) radius: f32,
    // per-instance clip rect [x,y,w,h] (logical) + the fragment's logical position,
    // compared in the fragment stage to clip scrolled content (SOUL §3.2).
    @location(4) clip: vec4<f32>,
    @location(5) logical: vec2<f32>,
};

// Discards the fragment when its logical position falls outside the clip rect. The
// unclipped sentinel spans all of logical space, so this is one always-pass branch
// for non-scrolled content (SOUL §3.2).
fn clip_discard(logical: vec2<f32>, clip: vec4<f32>) -> bool {
    return logical.x < clip.x || logical.y < clip.y
        || logical.x > clip.x + clip.z || logical.y > clip.y + clip.w;
}

@vertex
fn vs_quad(@builtin(vertex_index) vid: u32,
           @location(0) rect: vec4<f32>,
           @location(1) color: vec4<f32>,
           @location(2) params: vec4<f32>,
           @location(3) clip: vec4<f32>) -> QuadOut {
    let s = u.params.x;
    let corner = unit_corner(vid);
    // Rotate the unit-quad corner about the rect centre by params.y (radians). At
    // rotation 0 (cos=1, sin=0) this is identical to the axis-aligned fast path, so
    // rects are unaffected; a line carries a nonzero angle to orient along A->B.
    let center = rect.xy + rect.zw * 0.5;
    let local_off = (corner - vec2<f32>(0.5, 0.5)) * rect.zw;
    let angle = params.y;
    let ca = cos(angle);
    let sa = sin(angle);
    let rotated = vec2<f32>(local_off.x * ca - local_off.y * sa,
                            local_off.x * sa + local_off.y * ca);
    let logical = center + rotated;                 // logical-space vertex position
    let pos_px = logical * s;                        // logical -> physical (§7.1)
    var out: QuadOut;
    out.pos = to_ndc(pos_px);
    out.color = color;
    // SDF is evaluated in the quad's *unrotated* local frame (physical), so the
    // rounded-rect distance is rotation-invariant and line caps round correctly.
    out.local = local_off * s;
    out.half_size = rect.zw * 0.5 * s;
    out.radius = params.x * s;
    out.clip = clip;
    out.logical = logical;
    return out;
}

@fragment
fn fs_quad(in: QuadOut) -> @location(0) vec4<f32> {
    if (clip_discard(in.logical, in.clip)) {
        discard;
    }
    let r = min(in.radius, min(in.half_size.x, in.half_size.y));
    let qd = abs(in.local) - (in.half_size - vec2<f32>(r, r));
    let outside = length(max(qd, vec2<f32>(0.0, 0.0)));
    let inside = min(max(qd.x, qd.y), 0.0);
    let sd = outside + inside - r;
    let cov = 1.0 - smoothstep(-0.5, 0.5, sd);
    let lin = srgb_to_linear(in.color.rgb);
    return vec4<f32>(lin, in.color.a * cov);
}

struct GlyphOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) clip: vec4<f32>,
    @location(3) logical: vec2<f32>,
};

@vertex
fn vs_glyph(@builtin(vertex_index) vid: u32,
            @location(0) rect: vec4<f32>,
            @location(1) atlas_uv: vec4<f32>,
            @location(2) color: vec4<f32>,
            @location(3) clip: vec4<f32>) -> GlyphOut {
    let s = u.params.x;
    let corner = unit_corner(vid);
    // logical glyph dest rect -> physical; the atlas bitmap was rasterized at the
    // physical size, so this lands 1:1 on the target and stays crisp (SOUL §7.1).
    let logical = rect.xy + corner * rect.zw;
    let pos_px = logical * s;
    var out: GlyphOut;
    out.pos = to_ndc(pos_px);
    out.color = color;
    let texel = atlas_uv.xy + corner * atlas_uv.zw;
    out.uv = texel / u.atlas_size;
    out.clip = clip;
    out.logical = logical;
    return out;
}

@fragment
fn fs_glyph(in: GlyphOut) -> @location(0) vec4<f32> {
    if (clip_discard(in.logical, in.clip)) {
        discard;
    }
    let cov = textureSample(atlas_tex, atlas_samp, in.uv).r;
    let lin = srgb_to_linear(in.color.rgb);
    return vec4<f32>(lin, in.color.a * cov);
}

// Image quads sample the RGBA image atlas (bound at group(1) by the image
// pipeline — same layout as the glyph atlas). The atlas is Rgba8UnormSrgb, so
// sampled texels are already linear; the instance colour is a tint (WHITE =
// as-authored), decoded from sRGB like every other instance colour (SOUL §7.2).
// UVs normalize by the image atlas dims riding u.params.yz.
@vertex
fn vs_image(@builtin(vertex_index) vid: u32,
            @location(0) rect: vec4<f32>,
            @location(1) atlas_uv: vec4<f32>,
            @location(2) color: vec4<f32>,
            @location(3) clip: vec4<f32>) -> GlyphOut {
    let s = u.params.x;
    let corner = unit_corner(vid);
    let logical = rect.xy + corner * rect.zw;
    let pos_px = logical * s;
    var out: GlyphOut;
    out.pos = to_ndc(pos_px);
    out.color = color;
    let texel = atlas_uv.xy + corner * atlas_uv.zw;
    out.uv = texel / vec2<f32>(u.params.y, u.params.z);
    out.clip = clip;
    out.logical = logical;
    return out;
}

@fragment
fn fs_image(in: GlyphOut) -> @location(0) vec4<f32> {
    if (clip_discard(in.logical, in.clip)) {
        discard;
    }
    let texel = textureSample(atlas_tex, atlas_samp, in.uv);
    let tint = srgb_to_linear(in.color.rgb);
    return vec4<f32>(texel.rgb * tint, texel.a * in.color.a);
}
"#;

// Encodes tightly-packed RGBA8 rows as a PNG byte vector (SOUL §7.2).
