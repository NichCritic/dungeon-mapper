#!/usr/bin/env python3
"""
Recipe-based dungeon preview renderer.

Each theme defines a recipe: an ordered list of render steps.
Each step is a named function with parameters. The engine walks
the list, passing shared context (canvas, masks, distance fields).

Shared infrastructure: noise, Voronoi, wall geometry, distance fields.
Theme-specific: floor style, invasion type, overlay types, exterior style.
"""

import shutil
import time
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter
from scipy.ndimage import distance_transform_edt

TILE = 64

LAYOUT = np.array([
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0],
    [0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1, 0],
    [0, 1, 1, 1, 1, 1, 0, 0, 1, 1, 1, 1, 0],
    [0, 0, 0, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0],
    [0, 0, 0, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0],
    [0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0],
    [0, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0],
    [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
], dtype=np.int32)

ROWS, COLS = LAYOUT.shape
PX_H = ROWS * TILE
PX_W = COLS * TILE


# ===================================================================
# NOISE PRIMITIVES
# ===================================================================

def _fade_v(t):
    return t * t * t * (t * (t * 6 - 15) + 10)

class VectorPerlinNoise:
    def __init__(self, seed=0):
        rng = np.random.RandomState(seed)
        self.perm = np.arange(256, dtype=np.int32)
        rng.shuffle(self.perm)
        self.perm = np.tile(self.perm, 2)
        angles = rng.uniform(0, 2 * np.pi, 256)
        self.grad_x = np.cos(angles)
        self.grad_y = np.sin(angles)

    def noise_2d(self, x_arr, y_arr):
        xi = np.floor(x_arr).astype(np.int32) & 255
        yi = np.floor(y_arr).astype(np.int32) & 255
        xf = x_arr - np.floor(x_arr)
        yf = y_arr - np.floor(y_arr)
        u = _fade_v(xf)
        v = _fade_v(yf)
        aa = self.perm[self.perm[xi] + yi]
        ab = self.perm[self.perm[xi] + yi + 1]
        ba = self.perm[self.perm[xi + 1] + yi]
        bb = self.perm[self.perm[xi + 1] + yi + 1]
        def gd(h, dx, dy):
            return self.grad_x[h % 256] * dx + self.grad_y[h % 256] * dy
        x1 = (1 - u) * gd(aa, xf, yf) + u * gd(ba, xf - 1, yf)
        x2 = (1 - u) * gd(ab, xf, yf - 1) + u * gd(bb, xf - 1, yf - 1)
        return (1 - v) * x1 + v * x2

    def fbm_2d(self, x_arr, y_arr, octaves=4, lacunarity=2.0, gain=0.5):
        result = np.zeros_like(x_arr, dtype=np.float64)
        amp, freq = 1.0, 1.0
        for _ in range(octaves):
            result += amp * self.noise_2d(x_arr * freq, y_arr * freq)
            amp *= gain; freq *= lacunarity
        return result


def noise(h, w, scale, octaves=4, seed=0):
    """Normalized [0,1] noise field."""
    pn = VectorPerlinNoise(seed)
    yy, xx = np.mgrid[0:h, 0:w]
    field = pn.fbm_2d(xx.astype(np.float64) / TILE * scale,
                       yy.astype(np.float64) / TILE * scale, octaves)
    mn, mx = field.min(), field.max()
    return (field - mn) / (mx - mn + 1e-8)


def voronoi(h, w, density=0.8, seed=0):
    """Returns (cell_values, edge_field) both [0,1]."""
    rng = np.random.RandomState(seed)
    n_pts = max(4, int(ROWS * COLS * density))
    pts = rng.uniform(0, 1, (n_pts, 2)) * np.array([w, h])
    cell_vals = rng.uniform(0.2, 1.0, n_pts)
    yy, xx = np.mgrid[0:h, 0:w]
    coords = np.stack([xx.ravel(), yy.ravel()], axis=1).astype(np.float64)
    chunk = 8192
    cells_flat = np.zeros(h * w); edge_flat = np.zeros(h * w)
    for s in range(0, h * w, chunk):
        e = min(s + chunk, h * w)
        d = np.sqrt(((coords[s:e, None, :] - pts[None, :, :]) ** 2).sum(axis=2))
        cells_flat[s:e] = cell_vals[np.argmin(d, axis=1)]
        p = np.partition(d, 2, axis=1)[:, :2]; p.sort(axis=1)
        edge_flat[s:e] = p[:, 1] - p[:, 0]
    cells = cells_flat.reshape(h, w)
    edges = edge_flat.reshape(h, w)
    mn, mx = edges.min(), edges.max()
    return cells, (edges - mn) / (mx - mn + 1e-8)


# ===================================================================
# RENDER CONTEXT — shared state passed to all steps
# ===================================================================

class RenderContext:
    def __init__(self):
        self.canvas = np.zeros((PX_H, PX_W, 4), dtype=np.float64)
        self.canvas[:, :, 3] = 255
        self.floor_mask = np.repeat(np.repeat(LAYOUT, TILE, axis=0), TILE, axis=1).astype(bool)
        self.void_mask = ~self.floor_mask
        self.inner_dist = distance_transform_edt(self.floor_mask)
        self.void_dist = distance_transform_edt(self.void_mask)

    def blend(self, color, alpha, mask=None):
        """Alpha-blend a solid color onto canvas where mask is True."""
        if mask is not None:
            alpha = np.where(mask, alpha, 0)
        for c in range(3):
            self.canvas[:, :, c] += (color[c] - self.canvas[:, :, c]) * alpha

    def darken(self, amount, mask=None):
        """Multiply-darken the canvas."""
        if mask is not None:
            amount = np.where(mask, amount, 0)
        for c in range(3):
            self.canvas[:, :, c] *= (1 - amount)

    def paint(self, color_arr, mask):
        """Set canvas RGB where mask is True. color_arr is (H,W,3) or (3,)."""
        for c in range(3):
            if color_arr.ndim == 1:
                self.canvas[:, :, c] = np.where(mask, np.clip(color_arr[c], 0, 255), self.canvas[:, :, c])
            else:
                self.canvas[:, :, c] = np.where(mask, np.clip(color_arr[:, :, c], 0, 255), self.canvas[:, :, c])


# ===================================================================
# GENERIC RENDER STEPS (shared across themes)
# ===================================================================

def step_ao_radial(ctx, radius=0.5, strength=0.45):
    """Ambient occlusion: darken floor near walls."""
    ao = np.clip(1.0 - ctx.inner_dist / (TILE * radius), 0, 1) ** 1.8 * strength
    ctx.darken(ao, ctx.floor_mask)


def step_lighting_radial(ctx, radius=2.5, strength=0.35):
    """Radial vignette: bright center, dark near walls."""
    brightness = np.clip(ctx.inner_dist / (TILE * radius), 0, 1) ** 0.6
    shadow = (1.0 - brightness) * strength
    shadow_col = np.array(ctx.theme.get('shadow_color', (0, 0, 0)), dtype=np.float64)
    shadow_masked = np.where(ctx.floor_mask, shadow, 0)
    for c in range(3):
        ctx.canvas[:, :, c] = ctx.canvas[:, :, c] * (1 - shadow_masked) + shadow_col[c] * shadow_masked


def step_walls_stone(ctx, thickness=6, face_height=20):
    """2.5D walls outside floor extent with stone texture."""
    void_mask = ctx.void_mask
    void_dist = ctx.void_dist
    wall_noise_field = noise(PX_H, PX_W, scale=2.0, octaves=2, seed=hash(ctx.theme['name']) & 0xFFFF)

    wall_top = np.array(ctx.theme['wall_top'], dtype=np.float64)
    face_lit = np.array(ctx.theme['wall_face_lit'], dtype=np.float64)
    face_dark = np.array(ctx.theme['wall_face_dark'], dtype=np.float64)

    # Wall stone texture
    wall_cells, wall_edges = voronoi(PX_H, PX_W, density=2.0, seed=700)
    wall_cells2, wall_edges2 = voronoi(PX_H, PX_W, density=0.6, seed=701)
    stone_var = (wall_cells - 0.5) * 22 + (wall_cells2 - 0.5) * 10
    mortar = np.maximum(
        np.clip(1.0 - wall_edges * 4.0, 0, 1) ** 1.5,
        np.clip(1.0 - wall_edges2 * 3.0, 0, 1) ** 1.5 * 0.6)

    # Inner faces
    for d in range(face_height):
        dist_val = thickness + 1 + d
        band = void_mask & (void_dist >= dist_val - 0.5) & (void_dist < dist_val + 0.5)
        t = (d / face_height) ** 0.7
        base = face_lit * (1 - t) + face_dark * t
        for c in range(3):
            color = base[c] + np.where(band, stone_var * (1 - t * 0.5), 0)
            color = color * (1 - mortar * 0.3 * (1 - t)) + face_dark[c] * mortar * 0.3 * (1 - t)
            color += np.where(band, (wall_noise_field - 0.5) * 8 * (1 - t), 0)
            ctx.canvas[:, :, c] = np.where(band, np.clip(color, 0, 255), ctx.canvas[:, :, c])

    # Crease
    crease_color = face_dark * 0.7
    for d in range(2):
        a = 0.6 * (1 - d * 0.4)
        dv = thickness + d
        crease = void_mask & (void_dist >= dv + 0.5) & (void_dist < dv + 1.5)
        ctx.blend(crease_color, a, crease)

    # Wall top border
    border = void_mask & (void_dist <= thickness)
    noise_var = (wall_noise_field - 0.5) * 10
    for c in range(3):
        base_val = wall_top[c] + noise_var + stone_var
        base_val = base_val * (1 - mortar * 0.4) + (wall_top[c] * 0.5) * mortar * 0.4
        ctx.canvas[:, :, c] = np.where(border, np.clip(base_val, 0, 255), ctx.canvas[:, :, c])


# ===================================================================
# FLOOR STEPS
# ===================================================================

def _make_tile(size, base, accent, contrast, seed):
    """Simple flat tile with subtle variation."""
    pn = VectorPerlinNoise(seed)
    rng = np.random.RandomState(seed)
    yy, xx = np.mgrid[0:size, 0:size]
    surface = pn.fbm_2d(xx.astype(np.float64) / size * 3, yy.astype(np.float64) / size * 3, octaves=2)
    surface = (surface - surface.min()) / (surface.max() - surface.min() + 1e-8)
    fine = pn.fbm_2d(xx.astype(np.float64) / size * 10 + 50, yy.astype(np.float64) / size * 10 + 50, octaves=2)
    fine = (fine - fine.min()) / (fine.max() - fine.min() + 1e-8)
    hue = rng.uniform(-0.15, 0.15)
    t = np.clip(0.5 + hue + (surface - 0.5) * 0.3, 0, 1)
    tile = np.zeros((size, size, 4), dtype=np.float64)
    for c in range(3):
        tile[:, :, c] = np.clip(base[c] * t + accent[c] * (1 - t) + (fine - 0.5) * contrast * 0.5, 0, 255)
    tile[:, :, 3] = 255
    return tile


def step_floor_sandstone(ctx, contrast=15):
    """Warm, flat sandstone tiles."""
    base = np.array(ctx.theme['floor_base'], dtype=np.float64)
    accent = np.array(ctx.theme['floor_accent'], dtype=np.float64)
    cells = [(r, c) for r in range(ROWS) for c in range(COLS) if LAYOUT[r, c] == 1]
    rng = np.random.RandomState(42)
    indices = list(range(128)); rng.shuffle(indices)
    tiles = [_make_tile(TILE, base, accent, contrast, seed=1000 + i) for i in range(128)]
    for idx, (r, c) in enumerate(cells):
        ctx.canvas[r*TILE:(r+1)*TILE, c*TILE:(c+1)*TILE] = tiles[indices[idx % 128]]


def step_floor_slate(ctx, contrast=20):
    """Cool gray-blue slate tiles (ice theme)."""
    step_floor_sandstone(ctx, contrast=contrast)  # same gen, different palette in theme


def step_floor_obsidian(ctx, contrast=12):
    """Dark volcanic tiles with glassy surface."""
    base = np.array(ctx.theme['floor_base'], dtype=np.float64)
    accent = np.array(ctx.theme['floor_accent'], dtype=np.float64)
    cells = [(r, c) for r in range(ROWS) for c in range(COLS) if LAYOUT[r, c] == 1]
    rng = np.random.RandomState(42)
    indices = list(range(128)); rng.shuffle(indices)
    tiles = []
    for i in range(128):
        pn = VectorPerlinNoise(2000 + i)
        r2 = np.random.RandomState(2000 + i)
        yy, xx = np.mgrid[0:TILE, 0:TILE]
        # Glassy obsidian: smoother, less grain, occasional bright flecks
        surface = pn.fbm_2d(xx.astype(np.float64) / TILE * 2, yy.astype(np.float64) / TILE * 2, octaves=2)
        surface = (surface - surface.min()) / (surface.max() - surface.min() + 1e-8)
        hue = r2.uniform(-0.1, 0.1)
        t = np.clip(0.5 + hue + (surface - 0.5) * 0.2, 0, 1)
        tile = np.zeros((TILE, TILE, 4), dtype=np.float64)
        for c in range(3):
            tile[:, :, c] = np.clip(base[c] * t + accent[c] * (1 - t), 0, 255)
        tile[:, :, 3] = 255
        tiles.append(tile)
    for idx, (r, c) in enumerate(cells):
        ctx.canvas[r*TILE:(r+1)*TILE, c*TILE:(c+1)*TILE] = tiles[indices[idx % 128]]


# ===================================================================
# GROUT STEPS
# ===================================================================

def _grout_line(ctx, py, px, strength, color):
    if 0 <= py < PX_H and 0 <= px < PX_W and ctx.floor_mask[py, px]:
        for c in range(3):
            ctx.canvas[py, px, c] = np.clip(
                ctx.canvas[py, px, c] * (1 - strength) + color[c] * strength, 0, 255)


def step_grout_mossy(ctx, width_range=(1, 3), moss_chance=0.4):
    """Variable-width grout with optional moss fill."""
    mortar = np.array(ctx.theme['mortar'], dtype=np.float64)
    moss = np.array(ctx.theme.get('moss_colors', [(40, 80, 30)])[0], dtype=np.float64)
    for r in range(ROWS):
        for c in range(COLS):
            if LAYOUT[r, c] != 1: continue
            x0, y0 = c * TILE, r * TILE
            for edge in ('h', 'v'):
                s = hash((r, c, edge)) & 0xFFFF
                erng = np.random.RandomState(s)
                w = erng.randint(width_range[0], width_range[1] + 1)
                dark = 0.5 + erng.uniform(0, 0.4)
                use_moss = erng.random() < moss_chance
                color = moss if use_moss else mortar
                strength_base = dark * (0.6 if use_moss else 1.0)
                for d in range(w):
                    strength = strength_base * (1 - d / max(w, 1) * 0.3)
                    if edge == 'h':
                        py = y0 + TILE - 1 - d
                        for px in range(x0, min(x0 + TILE, PX_W)):
                            _grout_line(ctx, py, px, strength, color)
                    else:
                        px = x0 + TILE - 1 - d
                        for py in range(y0, min(y0 + TILE, PX_H)):
                            _grout_line(ctx, py, px, strength, color)


def step_grout_frozen(ctx, width_range=(1, 2)):
    """Thin grout with frost crystals — ice theme."""
    mortar = np.array(ctx.theme['mortar'], dtype=np.float64)
    frost = np.array(ctx.theme.get('moss_colors', [(180, 200, 220)])[0], dtype=np.float64)
    for r in range(ROWS):
        for c in range(COLS):
            if LAYOUT[r, c] != 1: continue
            x0, y0 = c * TILE, r * TILE
            for edge in ('h', 'v'):
                s = hash((r, c, edge)) & 0xFFFF
                erng = np.random.RandomState(s)
                w = erng.randint(width_range[0], width_range[1] + 1)
                dark = 0.3 + erng.uniform(0, 0.3)
                # Frost fills some grout lines (lighter, not darker)
                use_frost = erng.random() < 0.3
                if use_frost:
                    color = frost
                    strength_base = 0.3
                else:
                    color = mortar
                    strength_base = dark
                for d in range(w):
                    strength = strength_base * (1 - d / max(w, 1) * 0.3)
                    if edge == 'h':
                        py = y0 + TILE - 1 - d
                        for px in range(x0, min(x0 + TILE, PX_W)):
                            _grout_line(ctx, py, px, strength, color)
                    else:
                        px = x0 + TILE - 1 - d
                        for py in range(y0, min(y0 + TILE, PX_H)):
                            _grout_line(ctx, py, px, strength, color)


def step_grout_cracked(ctx, width_range=(1, 2), glow_chance=0.15):
    """Dark cracked grout with occasional lava glow — volcano theme."""
    mortar = np.array(ctx.theme['mortar'], dtype=np.float64)
    glow = np.array([220, 100, 20], dtype=np.float64)
    for r in range(ROWS):
        for c in range(COLS):
            if LAYOUT[r, c] != 1: continue
            x0, y0 = c * TILE, r * TILE
            for edge in ('h', 'v'):
                s = hash((r, c, edge)) & 0xFFFF
                erng = np.random.RandomState(s)
                w = erng.randint(width_range[0], width_range[1] + 1)
                dark = 0.6 + erng.uniform(0, 0.3)
                use_glow = erng.random() < glow_chance
                for d in range(w):
                    strength = dark * (1 - d / max(w, 1) * 0.3)
                    if use_glow and d == 0:
                        color = glow; strength = 0.4
                    else:
                        color = mortar
                    if edge == 'h':
                        py = y0 + TILE - 1 - d
                        for px in range(x0, min(x0 + TILE, PX_W)):
                            _grout_line(ctx, py, px, strength, color)
                    else:
                        px = x0 + TILE - 1 - d
                        for py in range(y0, min(y0 + TILE, PX_H)):
                            _grout_line(ctx, py, px, strength, color)


# ===================================================================
# INVASION STEPS (theme-specific edge effects)
# ===================================================================

def step_vine_tendrils(ctx, reach=1.8, density=0.45):
    """Organic vine tendrils creeping from exterior into rooms."""
    n1 = noise(PX_H, PX_W, scale=1.5, octaves=4, seed=800)
    n2 = noise(PX_H, PX_W, scale=4.0, octaves=3, seed=801)
    n3 = noise(PX_H, PX_W, scale=0.5, octaves=2, seed=802)

    prox = np.clip(1.0 - ctx.inner_dist / (TILE * reach), 0, 1)
    tendril = prox * (0.3 + 0.7 * n1) * (0.5 + 0.5 * n2) * (0.2 + 0.8 * np.clip(n3 * 1.5, 0, 1))
    vine_mask = np.clip((tendril - 0.25) * 4.0, 0, 1)
    vine_mask[ctx.void_mask] = 0

    colors = [np.array(mc, dtype=np.float64) for mc in ctx.theme['moss_colors']]
    if not colors: return
    ctx.blend(colors[0], vine_mask * 0.6, ctx.floor_mask)
    if len(colors) > 1:
        highlight = np.clip((tendril - 0.4) * 5.0, 0, 1) * vine_mask
        ctx.blend(colors[1], highlight * 0.4, ctx.floor_mask)
    if len(colors) > 2:
        edge_shadow = vine_mask * np.clip(1.0 - tendril * 2, 0, 1)
        ctx.blend(colors[2] * 0.5, edge_shadow * 0.3, ctx.floor_mask)


def step_frost_creep(ctx, reach=1.2, density=0.3):
    """Crystalline frost spreading from cold walls. Feathery, branching."""
    # Higher frequency noise for crystalline/feathery look
    n1 = noise(PX_H, PX_W, scale=2.5, octaves=4, seed=810)
    n2 = noise(PX_H, PX_W, scale=6.0, octaves=3, seed=811)
    n3 = noise(PX_H, PX_W, scale=1.0, octaves=2, seed=812)

    prox = np.clip(1.0 - ctx.inner_dist / (TILE * reach), 0, 1)
    # Crystalline: sharper threshold, more on/off
    crystal = prox * n1 * (0.6 + 0.4 * n2)
    crystal *= (0.3 + 0.7 * np.clip(n3 * 1.5, 0, 1))
    frost = np.clip((crystal - 0.15) * 6.0, 0, 1)  # sharper edges
    frost[ctx.void_mask] = 0

    colors = [np.array(mc, dtype=np.float64) for mc in ctx.theme['moss_colors']]
    if not colors: return
    # Frost is lighter, not darker — additive feel
    ctx.blend(colors[0], frost * 0.35, ctx.floor_mask)
    if len(colors) > 1:
        # Bright rime highlights on thickest frost
        rime = np.clip((crystal - 0.3) * 8.0, 0, 1) * frost
        ctx.blend(colors[1], rime * 0.3, ctx.floor_mask)


def step_lava_seep(ctx, reach=1.5, intensity=0.6, crack_density=1.8):
    """Lava seeping through cracks near walls. Glowing cores, dark edges."""
    # Use Voronoi cracks for lava channels
    _, crack_edges = voronoi(PX_H, PX_W, density=crack_density, seed=820)
    cracks = np.clip(1.0 - crack_edges * 4.0, 0, 1) ** 2

    prox = np.clip(1.0 - ctx.inner_dist / (TILE * reach), 0, 1)
    n_variation = noise(PX_H, PX_W, scale=0.6, octaves=3, seed=821)

    # Lava flows in cracks near walls
    lava = cracks * prox * (0.4 + 0.6 * n_variation)
    lava_mask = np.clip(lava * 3.0, 0, 1)
    lava_mask[ctx.void_mask] = 0

    colors = [np.array(mc, dtype=np.float64) for mc in ctx.theme['moss_colors']]
    if not colors: return

    # Dark scorch around lava
    scorch = np.clip(prox * 1.5, 0, 1) * 0.2
    ctx.darken(scorch, ctx.floor_mask)

    # Hot lava glow (bright core)
    ctx.blend(colors[0], lava_mask * intensity * 0.5, ctx.floor_mask)
    # Bright highlights in the crack centers
    if len(colors) > 1:
        bright = np.clip((lava - 0.3) * 5.0, 0, 1)
        ctx.blend(colors[1], bright * intensity * 0.4, ctx.floor_mask)


# ===================================================================
# OVERLAY STEPS
# ===================================================================

def step_stains_organic(ctx, coverage=0.3):
    """Warm dirt/water stains."""
    stain = np.array(ctx.theme['stain'], dtype=np.float64)
    n = noise(PX_H, PX_W, scale=0.4, octaves=4, seed=300)
    alpha = np.clip(n * 1.8 - 0.6, 0, 1) ** 1.2 * coverage
    ctx.blend(stain, alpha, ctx.floor_mask)


def step_stains_soot(ctx, coverage=0.25):
    """Dark scorch/soot stains — volcano theme."""
    stain = np.array(ctx.theme['stain'], dtype=np.float64)
    n = noise(PX_H, PX_W, scale=0.5, octaves=3, seed=305)
    alpha = np.clip(n * 2.0 - 0.7, 0, 1) ** 1.5 * coverage
    ctx.blend(stain, alpha, ctx.floor_mask)


def step_cracks_fine(ctx, density=1.5):
    """Fine crack network."""
    _, edges = voronoi(PX_H, PX_W, density=density, seed=400)
    cracks = np.clip(1.0 - edges * 5.0, 0, 1) ** 3
    sparse = noise(PX_H, PX_W, scale=0.5, octaves=2, seed=401)
    cracks *= np.clip(sparse * 2.5 - 1.0, 0, 1)
    mortar = np.array(ctx.theme['mortar'], dtype=np.float64) * 0.6
    ctx.blend(mortar, cracks * 0.4, ctx.floor_mask)


# ===================================================================
# EXTERIOR STEPS
# ===================================================================

def step_exterior_foliage(ctx, density=2.0):
    """Dense dark foliage exterior — jungle."""
    cells_sm, edges_sm = voronoi(PX_H, PX_W, density=density, seed=600)
    cells_md, edges_md = voronoi(PX_H, PX_W, density=0.5, seed=601)
    n1 = noise(PX_H, PX_W, scale=0.8, octaves=4, seed=602)
    n2 = noise(PX_H, PX_W, scale=3.0, octaves=3, seed=603)
    n3 = noise(PX_H, PX_W, scale=8.0, octaves=2, seed=604)
    base = np.array(ctx.theme['exterior'], dtype=np.float64)
    accent = np.array(ctx.theme['exterior_accent'], dtype=np.float64)
    detail = np.array(ctx.theme['exterior_detail'], dtype=np.float64)
    for c in range(3):
        color = base[c] * (0.4 + 0.6 * cells_md) + accent[c] * (0.6 - 0.6 * cells_md)
        color += (cells_sm - 0.5) * 20 + (n1 - 0.5) * 18 + (n2 - 0.5) * 12 + (n3 - 0.5) * 6
        edge_sm = np.clip(1.0 - edges_sm * 5.0, 0, 1) ** 2
        edge_md = np.clip(1.0 - edges_md * 3.0, 0, 1) ** 1.5
        color = color * (1 - edge_sm * 0.4) + accent[c] * 0.5 * edge_sm
        color = color * (1 - edge_md * 0.3) + detail[c] * 0.4 * edge_md
        ctx.canvas[:, :, c] = np.where(ctx.void_mask, np.clip(color, 0, 255), ctx.canvas[:, :, c])


def step_exterior_frozen(ctx, density=1.5):
    """Dark icy void — ice theme."""
    cells, edges = voronoi(PX_H, PX_W, density=density, seed=610)
    n1 = noise(PX_H, PX_W, scale=1.0, octaves=4, seed=611)
    n2 = noise(PX_H, PX_W, scale=4.0, octaves=2, seed=612)
    base = np.array(ctx.theme['exterior'], dtype=np.float64)
    accent = np.array(ctx.theme['exterior_accent'], dtype=np.float64)
    detail = np.array(ctx.theme['exterior_detail'], dtype=np.float64)
    for c in range(3):
        color = base[c] * cells + accent[c] * (1 - cells)
        color += (n1 - 0.5) * 15 + (n2 - 0.5) * 8
        edge_dark = np.clip(1.0 - edges * 3.5, 0, 1) ** 1.5
        color = color * (1 - edge_dark * 0.5) + detail[c] * 0.3 * edge_dark
        ctx.canvas[:, :, c] = np.where(ctx.void_mask, np.clip(color, 0, 255), ctx.canvas[:, :, c])


def step_exterior_volcanic(ctx, density=1.5):
    """Dark volcanic rock with subtle magma glow — volcano theme."""
    cells, edges = voronoi(PX_H, PX_W, density=density, seed=620)
    n1 = noise(PX_H, PX_W, scale=0.6, octaves=4, seed=621)
    n2 = noise(PX_H, PX_W, scale=3.0, octaves=3, seed=622)
    base = np.array(ctx.theme['exterior'], dtype=np.float64)
    accent = np.array(ctx.theme['exterior_accent'], dtype=np.float64)
    detail = np.array(ctx.theme['exterior_detail'], dtype=np.float64)
    # Magma glow in deepest cracks
    glow = np.array([120, 35, 8], dtype=np.float64)
    for c in range(3):
        color = base[c] * cells + accent[c] * (1 - cells)
        color += (n1 - 0.5) * 12 + (n2 - 0.5) * 8
        edge_dark = np.clip(1.0 - edges * 3.0, 0, 1) ** 1.5
        # Glow in crack bottoms
        color = color * (1 - edge_dark * 0.4) + glow[c] * edge_dark * 0.3
        color += (detail[c] - base[c]) * n2 * 0.2
        ctx.canvas[:, :, c] = np.where(ctx.void_mask, np.clip(color, 0, 255), ctx.canvas[:, :, c])


# ===================================================================
# RECIPES
# ===================================================================

RECIPES = {
    'jungle': {
        'name': 'Jungle Temple',
        'theme': {
            'floor_base': (155, 142, 118),
            'floor_accent': (135, 122, 98),
            'mortar': (65, 58, 42),
            'moss_colors': [(45, 85, 32), (62, 110, 42), (30, 55, 22)],
            'stain': (90, 82, 60),
            'wall_top': (75, 68, 52),
            'wall_face_lit': (85, 78, 60),
            'wall_face_dark': (30, 25, 18),
            'exterior': (12, 25, 10),
            'exterior_accent': (6, 14, 5),
            'exterior_detail': (18, 35, 12),
            'shadow_color': (20, 22, 15),
            'name': 'Jungle Temple',
        },
        'steps': [
            (step_exterior_foliage, {'density': 2.0}),
            (step_floor_sandstone, {'contrast': 15}),
            (step_grout_mossy, {'width_range': (1, 3), 'moss_chance': 0.4}),
            (step_stains_organic, {'coverage': 0.25}),
            (step_cracks_fine, {'density': 1.2}),
            (step_vine_tendrils, {'reach': 1.8, 'density': 0.45}),
            (step_ao_radial, {'radius': 0.5, 'strength': 0.45}),
            (step_lighting_radial, {'radius': 2.5, 'strength': 0.35}),
            (step_walls_stone, {'thickness': 6, 'face_height': 20}),
        ],
    },
    'ice': {
        'name': 'Frozen Caverns',
        'theme': {
            'floor_base': (162, 178, 195),
            'floor_accent': (125, 148, 172),
            'mortar': (95, 115, 142),
            'moss_colors': [(175, 205, 225), (210, 230, 245), (155, 185, 210)],
            'stain': (105, 128, 155),
            'wall_top': (120, 135, 155),
            'wall_face_lit': (130, 148, 168),
            'wall_face_dark': (50, 65, 88),
            'exterior': (18, 28, 42),
            'exterior_accent': (10, 16, 28),
            'exterior_detail': (25, 38, 55),
            'shadow_color': (15, 22, 40),
            'name': 'Frozen Caverns',
        },
        'steps': [
            (step_exterior_frozen, {'density': 1.5}),
            (step_floor_slate, {'contrast': 20}),
            (step_grout_frozen, {'width_range': (1, 2)}),
            (step_cracks_fine, {'density': 1.0}),
            (step_frost_creep, {'reach': 1.2, 'density': 0.3}),
            (step_ao_radial, {'radius': 0.5, 'strength': 0.4}),
            (step_lighting_radial, {'radius': 2.5, 'strength': 0.3}),
            (step_walls_stone, {'thickness': 6, 'face_height': 18}),
        ],
    },
    'volcano': {
        'name': 'Infernal Depths',
        'theme': {
            'floor_base': (52, 38, 32),
            'floor_accent': (35, 22, 18),
            'mortar': (25, 15, 10),
            'moss_colors': [(185, 62, 18), (225, 125, 28), (145, 42, 12)],
            'stain': (40, 18, 8),
            'wall_top': (38, 28, 22),
            'wall_face_lit': (50, 38, 30),
            'wall_face_dark': (18, 12, 8),
            'exterior': (12, 4, 2),
            'exterior_accent': (30, 8, 3),
            'exterior_detail': (45, 12, 5),
            'shadow_color': (5, 2, 1),
            'name': 'Infernal Depths',
        },
        'steps': [
            (step_exterior_volcanic, {'density': 1.5}),
            (step_floor_obsidian, {'contrast': 12}),
            (step_grout_cracked, {'width_range': (1, 2), 'glow_chance': 0.15}),
            (step_stains_soot, {'coverage': 0.2}),
            (step_lava_seep, {'reach': 1.5, 'intensity': 0.6, 'crack_density': 1.8}),
            (step_ao_radial, {'radius': 0.5, 'strength': 0.5}),
            (step_lighting_radial, {'radius': 2.5, 'strength': 0.4}),
            (step_walls_stone, {'thickness': 6, 'face_height': 22}),
        ],
    },
}


# ===================================================================
# RECIPE RUNNER
# ===================================================================

def run_recipe(recipe_name, output_path):
    recipe = RECIPES[recipe_name]
    print(f"\n  {recipe['name']}:")

    ctx = RenderContext()
    ctx.theme = recipe['theme']

    for step_fn, kwargs in recipe['steps']:
        name = step_fn.__name__.replace('step_', '')
        print(f"    {name}...", end='', flush=True)
        step_fn(ctx, **kwargs)
        print(" done")

    canvas = Image.fromarray(np.clip(ctx.canvas, 0, 255).astype(np.uint8), 'RGBA')
    canvas.save(output_path)
    print(f"  -> {output_path}")
    return canvas


def backup_previous(preview_dir, themes):
    for name in themes + ['comparison']:
        src = preview_dir / (f'{name}_preview.png' if name != 'comparison' else 'comparison.png')
        dst = preview_dir / (f'previous_{name}_preview.png' if name != 'comparison' else 'previous_comparison.png')
        if src.exists():
            shutil.copy2(src, dst)
    print("  Backed up previous previews")


def main():
    t0 = time.time()
    preview_dir = Path(__file__).resolve().parent.parent.parent / 'assets' / 'previews'
    preview_dir.mkdir(parents=True, exist_ok=True)

    themes = ['jungle', 'ice', 'volcano']
    backup_previous(preview_dir, themes)

    previews = []
    for name in themes:
        img = run_recipe(name, preview_dir / f'{name}_preview.png')
        previews.append(img)

    if previews:
        gap = 8
        total_w = sum(p.width for p in previews) + (len(previews) - 1) * gap
        max_h = max(p.height for p in previews)
        comp = Image.new('RGBA', (total_w, max_h), (20, 20, 20, 255))
        x = 0
        for p in previews:
            comp.paste(p, (x, 0))
            x += p.width + gap
        comp.save(preview_dir / 'comparison.png')
        print(f"\nComparison: {preview_dir / 'comparison.png'}")

    print(f"Total: {time.time() - t0:.1f}s")


if __name__ == '__main__':
    main()
