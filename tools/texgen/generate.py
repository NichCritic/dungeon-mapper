#!/usr/bin/env python3
"""
Procedural texture generator for dungeon-mapper asset packs.

Generates multi-layer alpha-blended tile sets at 64x64px for themed dungeon rendering.
2.5D style: walls have a top face and visible south/east faces for depth.
Shadows are computed at render time from map geometry (not baked into tiles).

Usage:
    python generate.py [--themes jungle,ice,volcano] [--size 64] [--output ../../assets/packs]
"""

import argparse
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

# ---------------------------------------------------------------------------
# Noise primitives
# ---------------------------------------------------------------------------

def _fade(t):
    return t * t * t * (t * (t * 6 - 15) + 10)

def _lerp(a, b, t):
    return a + t * (b - a)

class PerlinNoise:
    """Simple 2D Perlin noise generator."""

    def __init__(self, seed=0):
        rng = np.random.RandomState(seed)
        self.perm = np.arange(256, dtype=int)
        rng.shuffle(self.perm)
        self.perm = np.tile(self.perm, 2)
        angles = rng.uniform(0, 2 * np.pi, 256)
        self.grads = np.stack([np.cos(angles), np.sin(angles)], axis=1)

    def _grad(self, hash_val, x, y):
        g = self.grads[hash_val % 256]
        return g[0] * x + g[1] * y

    def noise(self, x, y):
        xi = int(np.floor(x)) & 255
        yi = int(np.floor(y)) & 255
        xf = x - np.floor(x)
        yf = y - np.floor(y)
        u = _fade(xf)
        v = _fade(yf)
        aa = self.perm[self.perm[xi] + yi]
        ab = self.perm[self.perm[xi] + yi + 1]
        ba = self.perm[self.perm[xi + 1] + yi]
        bb = self.perm[self.perm[xi + 1] + yi + 1]
        x1 = _lerp(self._grad(aa, xf, yf), self._grad(ba, xf - 1, yf), u)
        x2 = _lerp(self._grad(ab, xf, yf - 1), self._grad(bb, xf - 1, yf - 1), u)
        return _lerp(x1, x2, v)

    def fbm(self, x, y, octaves=4, lacunarity=2.0, gain=0.5):
        value = 0.0
        amplitude = 1.0
        frequency = 1.0
        for _ in range(octaves):
            value += amplitude * self.noise(x * frequency, y * frequency)
            amplitude *= gain
            frequency *= lacunarity
        return value


def noise_field(size, scale=4.0, octaves=4, seed=0):
    """Generate a 2D noise field normalized to [0, 1]."""
    pn = PerlinNoise(seed)
    field = np.zeros((size, size), dtype=np.float64)
    for y in range(size):
        for x in range(size):
            field[y, x] = pn.fbm(x / size * scale, y / size * scale, octaves)
    mn, mx = field.min(), field.max()
    if mx - mn > 1e-8:
        field = (field - mn) / (mx - mn)
    return field


def noise_field_rect(h, w, scale=4.0, octaves=4, seed=0):
    """Generate a rectangular noise field normalized to [0, 1]."""
    pn = PerlinNoise(seed)
    field = np.zeros((h, w), dtype=np.float64)
    for y in range(h):
        for x in range(w):
            field[y, x] = pn.fbm(x / w * scale, y / h * scale, octaves)
    mn, mx = field.min(), field.max()
    if mx - mn > 1e-8:
        field = (field - mn) / (mx - mn)
    return field


def voronoi_field(size, n_points=12, seed=0):
    """Generate a Voronoi distance field (edge-highlighting), normalized [0, 1]."""
    rng = np.random.RandomState(seed)
    pts_base = rng.uniform(0, size, (n_points, 2))
    pts = []
    for dx in (-size, 0, size):
        for dy in (-size, 0, size):
            pts.append(pts_base + np.array([dx, dy]))
    pts = np.concatenate(pts, axis=0)

    yy, xx = np.mgrid[0:size, 0:size]
    coords = np.stack([xx.ravel(), yy.ravel()], axis=1).astype(np.float64)
    dists = np.sqrt(((coords[:, None, :] - pts[None, :, :]) ** 2).sum(axis=2))
    sorted_dists = np.sort(dists, axis=1)
    d1 = sorted_dists[:, 0].reshape(size, size)
    d2 = sorted_dists[:, 1].reshape(size, size)

    edge = d2 - d1
    mn, mx = edge.min(), edge.max()
    if mx - mn > 1e-8:
        edge = (edge - mn) / (mx - mn)
    return edge


def cell_noise(size, n_points=16, seed=0):
    """Voronoi cell IDs — gives each cell a flat random value."""
    rng = np.random.RandomState(seed)
    pts_base = rng.uniform(0, size, (n_points, 2))
    cell_values = rng.uniform(0.3, 1.0, n_points)
    pts = []
    vals = []
    for dx in (-size, 0, size):
        for dy in (-size, 0, size):
            pts.append(pts_base + np.array([dx, dy]))
            vals.append(cell_values)
    pts = np.concatenate(pts, axis=0)
    vals = np.concatenate(vals)

    yy, xx = np.mgrid[0:size, 0:size]
    coords = np.stack([xx.ravel(), yy.ravel()], axis=1).astype(np.float64)
    dists = np.sqrt(((coords[:, None, :] - pts[None, :, :]) ** 2).sum(axis=2))
    nearest = np.argmin(dists, axis=1)
    result = vals[nearest].reshape(size, size)
    return result


# ---------------------------------------------------------------------------
# Image helpers
# ---------------------------------------------------------------------------

def field_to_rgba(field, color, alpha_scale=1.0):
    """Convert a [0,1] field to an RGBA image using the field as alpha."""
    h, w = field.shape[:2]
    img = np.zeros((h, w, 4), dtype=np.uint8)
    img[:, :, 0] = color[0]
    img[:, :, 1] = color[1]
    img[:, :, 2] = color[2]
    img[:, :, 3] = np.clip(field * 255 * alpha_scale, 0, 255).astype(np.uint8)
    return Image.fromarray(img, 'RGBA')


def solid_rgba(size, color):
    """Solid color RGBA image."""
    img = np.zeros((size, size, 4), dtype=np.uint8)
    img[:, :, 0] = color[0]
    img[:, :, 1] = color[1]
    img[:, :, 2] = color[2]
    img[:, :, 3] = color[3] if len(color) > 3 else 255
    return Image.fromarray(img, 'RGBA')


# ---------------------------------------------------------------------------
# AO / edge masks
# ---------------------------------------------------------------------------

def make_ao_mask(size, direction, falloff=0.4):
    """
    Wall-proximity ambient occlusion mask.
    direction: 'n', 's', 'e', 'w', 'ne', 'nw', 'se', 'sw'
    """
    alpha = np.zeros((size, size), dtype=np.float64)
    extent = int(size * falloff)

    if 'n' in direction:
        for y in range(extent):
            alpha[y, :] = np.maximum(alpha[y, :], 1.0 - y / extent)
    if 's' in direction:
        for y in range(extent):
            alpha[size - 1 - y, :] = np.maximum(alpha[size - 1 - y, :], 1.0 - y / extent)
    if 'w' in direction:
        for x in range(extent):
            alpha[:, x] = np.maximum(alpha[:, x], 1.0 - x / extent)
    if 'e' in direction:
        for x in range(extent):
            alpha[:, size - 1 - x] = np.maximum(alpha[:, size - 1 - x], 1.0 - x / extent)

    pn = noise_field(size, scale=6.0, octaves=2, seed=hash(direction) & 0xFFFF)
    alpha = alpha * (0.85 + 0.15 * pn)
    alpha = alpha ** 1.5

    return field_to_rgba(alpha, (0, 0, 0), alpha_scale=0.6)


# ---------------------------------------------------------------------------
# Light / atmosphere
# ---------------------------------------------------------------------------

def make_light_radial(size, color=(255, 200, 120)):
    """Radial light overlay."""
    yy, xx = np.mgrid[0:size, 0:size]
    cx, cy = size / 2, size / 2
    dist = np.sqrt((xx - cx) ** 2 + (yy - cy) ** 2) / (size / 2)
    alpha = np.clip(1.0 - dist, 0, 1) ** 2
    return field_to_rgba(alpha, color, alpha_scale=0.5)


def make_fog(size, seed=42, color=(180, 180, 200)):
    """Wispy fog overlay."""
    n = noise_field(size, scale=3.0, octaves=3, seed=seed)
    fog = np.clip((n - 0.4) * 3.0, 0, 1)
    return field_to_rgba(fog, color, alpha_scale=0.3)


# ---------------------------------------------------------------------------
# Theme definitions
# ---------------------------------------------------------------------------

THEMES = {
    'jungle': {
        'name': 'Jungle Temple',
        'description': 'Overgrown stone ruins reclaimed by the jungle',
        'palette': {
            'floor_base': (80, 75, 65),
            'floor_accent': (60, 55, 45),
            'mortar': (50, 45, 35),
            'moss': (45, 85, 35),
            'moss_light': (65, 110, 45),
            'stain': (55, 50, 40),
            'wall_top': (95, 90, 78),
            'wall_face_lit': (75, 70, 58),
            'wall_face_dark': (35, 30, 22),
            'wall_mortar': (45, 40, 30),
            'exterior': (25, 45, 20),
            'exterior_accent': (15, 30, 12),
            'fog_color': (120, 140, 110),
            'light_color': (200, 190, 140),
        },
        'floor_n_cells': 10,
        'moss_coverage': 0.5,
        'wall_height': 0.4,  # fraction of tile used for wall face
    },
    'ice': {
        'name': 'Frozen Caverns',
        'description': 'Ancient halls locked in permafrost',
        'palette': {
            'floor_base': (160, 175, 190),
            'floor_accent': (130, 150, 170),
            'mortar': (100, 120, 145),
            'moss': (170, 200, 220),
            'moss_light': (200, 225, 240),
            'stain': (110, 130, 155),
            'wall_top': (185, 195, 210),
            'wall_face_lit': (140, 155, 175),
            'wall_face_dark': (60, 75, 100),
            'wall_mortar': (90, 105, 130),
            'exterior': (40, 55, 75),
            'exterior_accent': (25, 35, 55),
            'fog_color': (180, 195, 215),
            'light_color': (160, 190, 230),
        },
        'floor_n_cells': 8,
        'moss_coverage': 0.35,
        'wall_height': 0.35,
    },
    'volcano': {
        'name': 'Infernal Depths',
        'description': 'Scorched obsidian halls above churning magma',
        'palette': {
            'floor_base': (45, 35, 30),
            'floor_accent': (60, 40, 30),
            'mortar': (30, 20, 15),
            'moss': (180, 60, 20),
            'moss_light': (220, 120, 30),
            'stain': (70, 30, 15),
            'wall_top': (70, 55, 45),
            'wall_face_lit': (55, 40, 32),
            'wall_face_dark': (20, 12, 8),
            'wall_mortar': (25, 15, 10),
            'exterior': (20, 8, 5),
            'exterior_accent': (60, 15, 5),
            'fog_color': (80, 30, 10),
            'light_color': (255, 140, 40),
        },
        'floor_n_cells': 14,
        'moss_coverage': 0.25,
        'wall_height': 0.45,
    },
}


# ---------------------------------------------------------------------------
# Floor tile generators
# ---------------------------------------------------------------------------

def generate_floor_base(size, theme, variant=0):
    """Base floor tile with Voronoi stone blocks and mortar lines."""
    pal = theme['palette']
    n_cells = theme['floor_n_cells']
    seed = variant * 1000 + 42

    cells = cell_noise(size, n_points=n_cells, seed=seed)
    edges = voronoi_field(size, n_points=n_cells, seed=seed)
    surface = noise_field(size, scale=8.0, octaves=4, seed=seed + 1)

    base_color = np.array(pal['floor_base'], dtype=np.float64)
    accent_color = np.array(pal['floor_accent'], dtype=np.float64)
    mortar_color = np.array(pal['mortar'], dtype=np.float64)

    img = np.zeros((size, size, 4), dtype=np.float64)
    t = cells
    img[:, :, 0] = base_color[0] * t + accent_color[0] * (1 - t)
    img[:, :, 1] = base_color[1] * t + accent_color[1] * (1 - t)
    img[:, :, 2] = base_color[2] * t + accent_color[2] * (1 - t)
    img[:, :, 3] = 255.0

    # Surface noise
    variation = (surface - 0.5) * 30
    for c in range(3):
        img[:, :, c] = np.clip(img[:, :, c] + variation, 0, 255)

    # Mortar lines at Voronoi edges
    mortar_mask = np.clip(1.0 - edges * 4.0, 0, 1)
    for c in range(3):
        img[:, :, c] = img[:, :, c] * (1 - mortar_mask * 0.7) + mortar_color[c] * mortar_mask * 0.7

    return Image.fromarray(np.clip(img, 0, 255).astype(np.uint8), 'RGBA')


def generate_floor_variation(size, theme, variant=0):
    """Overlay: moss/frost/ember patches."""
    pal = theme['palette']
    seed = variant * 1000 + 100
    coverage = theme['moss_coverage']

    patch_noise = noise_field(size, scale=3.0, octaves=3, seed=seed)
    detail_noise = noise_field(size, scale=8.0, octaves=2, seed=seed + 1)

    threshold = 1.0 - coverage
    patch_mask = np.clip((patch_noise - threshold) / (1.0 - threshold + 1e-8), 0, 1)
    patch_mask *= (0.7 + 0.3 * detail_noise)

    if variant % 3 == 0:
        color = pal['moss']
        alpha_scale = 0.75
    elif variant % 3 == 1:
        color = pal['moss_light']
        alpha_scale = 0.6
    else:
        color = pal['stain']
        stain_noise = noise_field(size, scale=5.0, octaves=4, seed=seed + 50)
        patch_mask = np.clip(stain_noise * 1.5 - 0.4, 0, 1)
        alpha_scale = 0.5

    return field_to_rgba(patch_mask, color, alpha_scale=alpha_scale)


def generate_crack_overlay(size, theme, variant=0):
    """Thin crack lines using high-freq Voronoi edges."""
    pal = theme['palette']
    seed = variant * 1000 + 200

    edges = voronoi_field(size, n_points=24, seed=seed)
    cracks = np.clip(1.0 - edges * 6.0, 0, 1)
    cracks = cracks ** 3

    mask_noise = noise_field(size, scale=4.0, octaves=2, seed=seed + 1)
    cracks *= np.clip(mask_noise * 2.0 - 0.5, 0, 1)

    return field_to_rgba(cracks, pal['mortar'], alpha_scale=0.6)


# ---------------------------------------------------------------------------
# 2.5D Wall tile generators
#
# Wall tiles are NOT full-cell occupants. They are border elements drawn
# along the edges of floor cells.  The asset pack provides:
#
#   wall_top.png        — top face, seen from above (full tile, used for
#                         wall cells that have no visible face)
#   wall_face_s.png     — south-facing wall face strip (full tile width,
#                         wall_height fraction tall, with stone + depth)
#   wall_face_e.png     — east-facing wall face strip (wall_height wide,
#                         full tile tall)
#   wall_corner_outer.png — outer convex corner piece (SE corner visible)
#   wall_corner_inner.png — inner concave corner fill
#
# The renderer composites these at the correct positions based on which
# edges of each cell border floor vs void.
# ---------------------------------------------------------------------------

def _stone_texture(h, w, seed, base_color, accent_color, mortar_color, n_cells=6):
    """Generate a stone-block texture at arbitrary dimensions."""
    # We need rectangular noise fields here
    pn = PerlinNoise(seed)
    # Simple cell noise for rectangle
    rng = np.random.RandomState(seed)
    pts = rng.uniform(0, max(h, w), (n_cells * 9, 2))
    # Adjust pts for w/h
    cell_vals = rng.uniform(0.3, 1.0, n_cells * 9)

    yy, xx = np.mgrid[0:h, 0:w]
    coords = np.stack([xx.ravel(), yy.ravel()], axis=1).astype(np.float64)
    dists = np.sqrt(((coords[:, None, :] - pts[None, :, :]) ** 2).sum(axis=2))
    nearest = np.argmin(dists, axis=1)
    cells = cell_vals[nearest].reshape(h, w)

    sorted_dists = np.sort(dists, axis=1)
    d1 = sorted_dists[:, 0].reshape(h, w)
    d2 = sorted_dists[:, 1].reshape(h, w)
    edges = d2 - d1
    mn, mx = edges.min(), edges.max()
    if mx - mn > 1e-8:
        edges = (edges - mn) / (mx - mn)

    # Surface noise
    surface = noise_field_rect(h, w, scale=8.0, octaves=3, seed=seed + 1)

    base = np.array(base_color, dtype=np.float64)
    accent = np.array(accent_color, dtype=np.float64)
    mortar = np.array(mortar_color, dtype=np.float64)

    img = np.zeros((h, w, 4), dtype=np.float64)
    t = cells
    for c in range(3):
        img[:, :, c] = base[c] * t + accent[c] * (1 - t)
    img[:, :, 3] = 255.0

    variation = (surface - 0.5) * 25
    for c in range(3):
        img[:, :, c] = np.clip(img[:, :, c] + variation, 0, 255)

    mortar_mask = np.clip(1.0 - edges * 5.0, 0, 1)
    for c in range(3):
        img[:, :, c] = img[:, :, c] * (1 - mortar_mask * 0.7) + mortar[c] * mortar_mask * 0.7

    return img


def generate_wall_top(size, theme):
    """Top face of wall — seen from above. Stone texture, slightly lighter."""
    pal = theme['palette']
    img = _stone_texture(
        size, size, seed=300,
        base_color=pal['wall_top'],
        accent_color=tuple(max(0, c - 15) for c in pal['wall_top']),
        mortar_color=pal['wall_mortar'],
        n_cells=8,
    )
    return Image.fromarray(np.clip(img, 0, 255).astype(np.uint8), 'RGBA')


def generate_wall_face_s(size, theme):
    """
    South-facing wall face. Full tile width, wall_height * size tall.
    Lit from top-left: lighter at top, darker at bottom.
    Has stone block texture with mortar.
    """
    pal = theme['palette']
    face_h = max(4, int(size * theme['wall_height']))

    img = _stone_texture(
        face_h, size, seed=310,
        base_color=pal['wall_face_lit'],
        accent_color=pal['wall_face_dark'],
        mortar_color=pal['wall_mortar'],
        n_cells=6,
    )

    # Apply vertical gradient: lighter at top, darker at bottom
    lit = np.array(pal['wall_face_lit'], dtype=np.float64)
    dark = np.array(pal['wall_face_dark'], dtype=np.float64)
    for y in range(face_h):
        t = (y / face_h) ** 0.6
        blend = lit * (1 - t) + dark * t
        for c in range(3):
            img[y, :, c] = img[y, :, c] * 0.5 + blend[c] * 0.5

    # Dark line at very bottom edge (shadow crease)
    crease = min(2, face_h // 4)
    for y in range(face_h - crease, face_h):
        t = (y - (face_h - crease)) / max(1, crease)
        img[y, :, :3] *= (1.0 - 0.4 * t)

    img = np.clip(img, 0, 255).astype(np.uint8)
    return Image.fromarray(img, 'RGBA')


def generate_wall_face_e(size, theme):
    """
    East-facing wall face. wall_height * size wide, full tile tall.
    Darker than south face (less direct light from top-left source).
    """
    pal = theme['palette']
    face_w = max(4, int(size * theme['wall_height']))

    # East face is dimmer
    face_lit = tuple(max(0, c - 15) for c in pal['wall_face_lit'])
    face_dark = tuple(max(0, c - 10) for c in pal['wall_face_dark'])

    img = _stone_texture(
        size, face_w, seed=320,
        base_color=face_lit,
        accent_color=face_dark,
        mortar_color=pal['wall_mortar'],
        n_cells=6,
    )

    # Horizontal gradient: lighter at left, darker at right
    for x in range(face_w):
        t = (x / face_w) ** 0.6
        blend_factor = 0.3 * t
        img[:, x, :3] *= (1.0 - blend_factor)

    # Dark line at right edge
    crease = min(2, face_w // 4)
    for x in range(face_w - crease, face_w):
        t = (x - (face_w - crease)) / max(1, crease)
        img[:, x, :3] *= (1.0 - 0.4 * t)

    img = np.clip(img, 0, 255).astype(np.uint8)
    return Image.fromarray(img, 'RGBA')


def generate_wall_corner_outer(size, theme):
    """
    Outer (convex) corner piece — SE corner.
    Combines a south face strip on top and east face strip on right,
    with a corner join.  Other rotations derived by the renderer.
    """
    pal = theme['palette']
    face_h = max(4, int(size * theme['wall_height']))
    face_w = max(4, int(size * theme['wall_height']))

    img = np.zeros((size, size, 4), dtype=np.float64)

    # Fill with wall top
    top_tex = _stone_texture(
        size, size, seed=330,
        base_color=pal['wall_top'],
        accent_color=tuple(max(0, c - 15) for c in pal['wall_top']),
        mortar_color=pal['wall_mortar'],
        n_cells=6,
    )
    img[:] = top_tex

    # Overlay south face at bottom
    s_face = _stone_texture(
        face_h, size, seed=331,
        base_color=pal['wall_face_lit'],
        accent_color=pal['wall_face_dark'],
        mortar_color=pal['wall_mortar'],
        n_cells=4,
    )
    for y in range(face_h):
        t = (y / face_h) ** 0.6
        s_face[y, :, :3] *= (1.0 - 0.3 * t)
    img[size - face_h:size, :, :] = s_face

    # Overlay east face on right
    e_face = _stone_texture(
        size, face_w, seed=332,
        base_color=tuple(max(0, c - 15) for c in pal['wall_face_lit']),
        accent_color=tuple(max(0, c - 10) for c in pal['wall_face_dark']),
        mortar_color=pal['wall_mortar'],
        n_cells=4,
    )
    for x in range(face_w):
        t = (x / face_w) ** 0.6
        e_face[:, x, :3] *= (1.0 - 0.3 * t)
    # Paste east face, but south face takes priority in overlap region
    for y in range(size - face_h):
        img[y, size - face_w:size, :] = e_face[y, :, :]
    # Corner overlap: blend
    for y in range(size - face_h, size):
        for x in range(size - face_w, size):
            ey = y
            img[y, x, :3] = (s_face[y - (size - face_h), x, :3] * 0.5 +
                              e_face[ey, x - (size - face_w), :3] * 0.5)
            img[y, x, 3] = 255.0

    return Image.fromarray(np.clip(img, 0, 255).astype(np.uint8), 'RGBA')


def generate_wall_corner_inner(size, theme):
    """
    Inner (concave) corner piece — the small triangular shadow
    where two wall faces meet at an interior corner.
    """
    pal = theme['palette']
    face_h = max(4, int(size * theme['wall_height']))

    # This is a small shadow/crease overlay for concave joins
    img = np.zeros((size, size, 4), dtype=np.float64)

    # Dark triangular shadow in the corner region
    for y in range(face_h):
        for x in range(face_h):
            # Distance from corner
            dy = y / face_h
            dx = x / face_h
            shadow = max(0, 1.0 - (dx + dy))
            img[size - face_h + y, size - face_h + x, :3] = 0
            img[size - face_h + y, size - face_h + x, 3] = shadow * 180

    return Image.fromarray(np.clip(img, 0, 255).astype(np.uint8), 'RGBA')


# ---------------------------------------------------------------------------
# Exterior tile generators
# ---------------------------------------------------------------------------

def generate_exterior(size, theme, variant=0):
    """Exterior/void fill with more texture variation."""
    pal = theme['palette']
    seed = variant * 1000 + 400

    base = np.array(pal['exterior'], dtype=np.float64)
    accent = np.array(pal['exterior_accent'], dtype=np.float64)

    n1 = noise_field(size, scale=4.0, octaves=4, seed=seed)
    n2 = noise_field(size, scale=12.0, octaves=3, seed=seed + 1)
    n3 = noise_field(size, scale=2.0, octaves=2, seed=seed + 2)

    # Multi-octave blending for more interesting exterior
    img = np.zeros((size, size, 4), dtype=np.float64)
    t = n1 * 0.5 + n2 * 0.3 + n3 * 0.2
    img[:, :, 0] = base[0] * t + accent[0] * (1 - t)
    img[:, :, 1] = base[1] * t + accent[1] * (1 - t)
    img[:, :, 2] = base[2] * t + accent[2] * (1 - t)

    # Add fine detail noise for texture
    detail = (n2 - 0.5) * 20
    for c in range(3):
        img[:, :, c] = np.clip(img[:, :, c] + detail, 0, 255)

    img[:, :, 3] = 255.0
    return Image.fromarray(np.clip(img, 0, 255).astype(np.uint8), 'RGBA')


# ---------------------------------------------------------------------------
# Grid overlay
# ---------------------------------------------------------------------------

def generate_grid_overlay(size):
    """Subtle grid line overlay — 1px lines at tile edges, alpha blended."""
    img = Image.new('RGBA', (size, size), (0, 0, 0, 0))
    draw = ImageDraw.Draw(img)
    # Bottom edge and right edge (so tiling shows continuous lines)
    alpha = 40
    color = (0, 0, 0, alpha)
    draw.line([(0, size - 1), (size - 1, size - 1)], fill=color, width=1)
    draw.line([(size - 1, 0), (size - 1, size - 1)], fill=color, width=1)
    return img


# ---------------------------------------------------------------------------
# Pack generation
# ---------------------------------------------------------------------------

def generate_pack(theme_name, size, output_dir):
    """Generate a complete asset pack for a theme."""
    theme = THEMES[theme_name]
    pal = theme['palette']
    pack_dir = Path(output_dir) / theme_name
    face_h = max(4, int(size * theme['wall_height']))
    face_w = face_h  # square aspect for wall thickness

    print(f"\n=== Generating '{theme['name']}' pack ({size}x{size}px, wall face {face_h}px) ===")

    dirs = ['floors', 'variations', 'cracks', 'edges', 'walls', 'exterior', 'atmosphere', 'grid']
    for d in dirs:
        (pack_dir / d).mkdir(parents=True, exist_ok=True)

    # --- Floor base tiles (4 variants) ---
    print("  Floors...", end='', flush=True)
    for i in range(4):
        generate_floor_base(size, theme, variant=i).save(pack_dir / 'floors' / f'base_{i}.png')
    print(" done")

    # --- Variation overlays (6 variants) ---
    print("  Variations...", end='', flush=True)
    for i in range(6):
        generate_floor_variation(size, theme, variant=i).save(pack_dir / 'variations' / f'overlay_{i}.png')
    print(" done")

    # --- Crack overlays (3 variants) ---
    print("  Cracks...", end='', flush=True)
    for i in range(3):
        generate_crack_overlay(size, theme, variant=i).save(pack_dir / 'cracks' / f'crack_{i}.png')
    print(" done")

    # --- AO edge masks (8 directions) ---
    print("  Edge AO...", end='', flush=True)
    for direction in ['n', 's', 'e', 'w', 'ne', 'nw', 'se', 'sw']:
        make_ao_mask(size, direction).save(pack_dir / 'edges' / f'ao_{direction}.png')
    print(" done")

    # --- 2.5D Wall tiles ---
    print("  Walls (2.5D)...", end='', flush=True)
    generate_wall_top(size, theme).save(pack_dir / 'walls' / 'top.png')
    generate_wall_face_s(size, theme).save(pack_dir / 'walls' / 'face_s.png')
    generate_wall_face_e(size, theme).save(pack_dir / 'walls' / 'face_e.png')
    generate_wall_corner_outer(size, theme).save(pack_dir / 'walls' / 'corner_outer.png')
    generate_wall_corner_inner(size, theme).save(pack_dir / 'walls' / 'corner_inner.png')
    print(" done")

    # --- Exterior tiles (3 variants) ---
    print("  Exterior...", end='', flush=True)
    for i in range(3):
        generate_exterior(size, theme, variant=i).save(pack_dir / 'exterior' / f'void_{i}.png')
    print(" done")

    # --- Grid overlay ---
    print("  Grid...", end='', flush=True)
    generate_grid_overlay(size).save(pack_dir / 'grid' / 'grid.png')
    print(" done")

    # --- Atmosphere ---
    print("  Atmosphere...", end='', flush=True)
    make_light_radial(size * 4, color=pal['light_color']).save(pack_dir / 'atmosphere' / 'light_radial.png')
    make_fog(size, seed=hash(theme_name) & 0xFFFF, color=pal['fog_color']).save(pack_dir / 'atmosphere' / 'fog.png')
    print(" done")

    # --- Manifest ---
    manifest = {
        'name': theme['name'],
        'description': theme['description'],
        'tile_size': size,
        'wall_face_height': face_h,
        'wall_face_width': face_w,
        'version': 2,
        'layers': {
            'floors': {
                'description': 'Base floor tiles, randomly selected per cell.',
                'files': [f'floors/base_{i}.png' for i in range(4)],
                'blend_mode': 'opaque',
            },
            'variations': {
                'description': 'Alpha overlays for moss/frost/ember. Applied by proximity to features.',
                'files': [f'variations/overlay_{i}.png' for i in range(6)],
                'blend_mode': 'alpha',
            },
            'cracks': {
                'description': 'Crack overlays, sparsely applied.',
                'files': [f'cracks/crack_{i}.png' for i in range(3)],
                'blend_mode': 'alpha',
            },
            'edges': {
                'description': 'Wall-proximity AO masks by direction.',
                'files': {d: f'edges/ao_{d}.png' for d in ['n', 's', 'e', 'w', 'ne', 'nw', 'se', 'sw']},
                'blend_mode': 'alpha',
            },
            'walls': {
                'description': '2.5D wall pieces. top = overhead view, face_s/face_e = visible depth faces, corners for joins.',
                'files': {
                    'top': 'walls/top.png',
                    'face_s': 'walls/face_s.png',
                    'face_e': 'walls/face_e.png',
                    'corner_outer': 'walls/corner_outer.png',
                    'corner_inner': 'walls/corner_inner.png',
                },
                'blend_mode': 'opaque',
                'note': 'face_s is full_width x wall_face_height. face_e is wall_face_width x full_height. Renderer places these at cell edges.',
            },
            'exterior': {
                'description': 'Void/exterior fill, tiled outside walls.',
                'files': [f'exterior/void_{i}.png' for i in range(3)],
                'blend_mode': 'opaque',
            },
            'grid': {
                'description': 'Subtle grid line overlay per cell.',
                'files': ['grid/grid.png'],
                'blend_mode': 'alpha',
            },
            'atmosphere': {
                'description': 'Light radial (4x tile size) and fog overlay.',
                'files': {
                    'light': 'atmosphere/light_radial.png',
                    'fog': 'atmosphere/fog.png',
                },
                'blend_mode': 'alpha',
            },
        },
    }
    with open(pack_dir / 'manifest.json', 'w') as f:
        json.dump(manifest, f, indent=2)

    total = sum(len(list((pack_dir / d).glob('*.png'))) for d in dirs)
    print(f"  -> {total} tiles written to {pack_dir}/")
    return pack_dir


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description='Generate dungeon-mapper texture packs')
    parser.add_argument('--themes', default='jungle,ice,volcano',
                        help='Comma-separated theme names (default: jungle,ice,volcano)')
    parser.add_argument('--size', type=int, default=64,
                        help='Tile size in pixels (default: 64)')
    parser.add_argument('--output', default=None,
                        help='Output directory (default: ../../assets/packs relative to this script)')
    args = parser.parse_args()

    if args.output is None:
        script_dir = Path(__file__).resolve().parent
        output = script_dir.parent.parent / 'assets' / 'packs'
    else:
        output = Path(args.output)

    themes = [t.strip() for t in args.themes.split(',')]
    for t in themes:
        if t not in THEMES:
            print(f"Unknown theme: {t}. Available: {', '.join(THEMES.keys())}")
            sys.exit(1)

    for t in themes:
        generate_pack(t, args.size, output)

    print(f"\nDone! Generated {len(themes)} packs in {output}/")


if __name__ == '__main__':
    main()
