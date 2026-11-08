// Generates magma_chamber.json: Riot's 2010 5v5 Magma Chamber ("Caldera") concept, Concept 04
// layout (positions read off the 1000 x 460 concept image, 21 game units per pixel, image top =
// north), fully mirrored left / right for fair play. West = frost side (blue), east = volcanic
// side (red): the same shapes, different art. Walls are built from scenery (Season 1 Crystal Scar
// and Twisted Treeline models, Map1 rock spires). Run: node recipes/magma_chamber_gen.cjs
const fs = require('fs');
const S = 21, W = 1000 * S, H = 500 * S, Y0 = 480;
const P = (x, y) => [Math.round(x * S), Math.round((Y0 - y) * S)];
const PS = (...a) => a.map(([x, y]) => P(x, y));
const MX = ([x, z]) => [W - x, z];
const steps = [];
const t = (tool, o = {}) => steps.push(Object.assign({ tool }, o));
const at = (x, y) => { const [X, Z] = P(x, y); return { x: X, z: Z }; };

// Scenery (Season 1 Crystal Scar = Season1-Map8, Twisted Treeline = Season1-Map4, Map1).
const M8 = k => 'Season1-Map8/' + k, M4 = k => 'Season1-Map4/' + k;
// Models are chosen by texture (names like "lambert #12" depend on what is loaded).
const ROCKS = ['@Season1-Map8:rock formations_1:80-400', '@Season1-Map8:rock formations_2:80-400', '@Season1-Map4:rock_uv1:120-400', '@Map1:stonehenge_revised:200-450'];
const FROZEN = ['@Season1-Map8:frozen_props_tree_02:350-1200'];
const CHARRED = ['@Season1-Map8:bunt_trees_charred_02:300-1200'];
const CLIFFS = ['@Season1-Map8:stone04b:300-900', '@Season1-Map8:sidestone:300-900'];
const TORCH = '@Season1-Map8:torch_tx_dm:250-400';
const RUNES = ['@Season1-Map8:decals_lane_runestones_a', '@Season1-Map8:decals_lane_runestones_b', '@Season1-Map8:decals_lane_runestones_f', '@Season1-Map8:decals_lane_runestones_e'];
const SMALL = ['@Season1-Map8:rock formations_2:40-110', '@Season1-Map8:rock formations_1:40-110'];
const CRYSTALS = ['@Season1-Map8:city_crystal_formations:40-200'];

t('new_map', { name: 'Magma Chamber', width: W, height: H, symmetry: 'mirror_x', fill: 'deep_water' });
t('look', {
  scenery_from: ['Map1', 'Season1-Map8', 'Season1-Map4', 'Season1-Map3'],
  lava: true, lava_from_x: W / 2, wall_style: 'league', dress_walls: false, mound: 50, river_depth: 160,
  sun_color: [215, 175, 165], ambient_color: [55, 55, 95],
  settings: { 'terrain.inibin|Fog*enable': '1', 'terrain.inibin|Fog*red': '70', 'terrain.inibin|Fog*green': '55', 'terrain.inibin|Fog*blue': '95', 'terrain.inibin|Fog*opacity': '0.18', 'Gamma.inibin|Gamma*Contrast': '1.08' },
});
// Ground layers (13): the frost half, the volcanic half, lanes, bases, plazas, paths, camps, rim.
const L = { volcanic: 0, burned: 1, road: 2, snow: 3, snow2: 4, orderFloor: 5, chaosFloor: 6, flagstone: 7, cracked: 8, orderDirt: 9, chaosDirt: 10, mine: 11, rim: 12, frostRoad: 13, scorchedRoad: 14 };
t('set_texture', { slot: 'ground0', texture: 'gen:ash', label: 'Volcanic rock' });
t('set_texture', { slot: 'ground1', texture: 'Chaos_dirt_moss_blend.dds', label: 'Burned ground' });
t('set_texture', { slot: 'ground2', texture: 'new_stone_road.dds', label: 'Stone road' });
t('set_texture', { slot: 'ground3', texture: 'gen:snow', label: 'Snow' });
t('fetch_texture', { query: 'dark volcanic rock', slot: 'ground0', label: 'Volcanic rock' });
t('fetch_texture', { query: 'burned ground', slot: 'ground1', label: 'Burned ground' });
t('fetch_texture', { query: 'aerial snow', slot: 'ground3', label: 'Snow' });
t('fetch_texture', { query: 'snow floor', slot: 'ground4', label: 'Packed snow' });
// Own textures, edited from Map1 / Season 1 / downloaded ones to fit each half.
t('make_texture', { name: 'mc_frost_base_floor', base: 'order_tile_floor.dds', slot: 'ground5', label: 'Frost base floor', steps: [
  { op: 'tint', color: [175, 205, 255], amount: 0.35 },
  { op: 'overlay', mask: 'dark', threshold: 0.33, softness: 0.1, color: [232, 240, 252], amount: 0.85 },
  { op: 'overlay', mask: 'noise', threshold: 0.68, softness: 0.12, scale: 3, texture: 'snow_floor_classic.png', amount: 0.75 },
  { op: 'speckle', color: [250, 252, 255], density: 0.03, amount: 0.6 } ] });
t('make_texture', { name: 'mc_ember_base_floor', base: 'Chaos_BlurFloor2.dds', slot: 'ground6', label: 'Ember base floor', steps: [
  { op: 'desaturate', amount: 0.4 },
  { op: 'tint', color: [105, 75, 70], amount: 0.4 },
  { op: 'brightness', factor: 0.8 },
  { op: 'overlay', mask: 'noise', threshold: 0.66, softness: 0.12, scale: 3, texture: 'cracked_red_ground_classic.png', amount: 0.6 },
  { op: 'cracks', color: [255, 115, 30], scale: 3, width: 0.018, amount: 0.9 } ] });
t('set_texture', { slot: 'ground7', texture: 'Terrain_MRT_Flagstone_1_TX_CM.dds', label: 'Flagstone' });
t('fetch_texture', { query: 'cracked red ground', slot: 'ground8', label: 'Cracked lava ground' });
t('make_texture', { name: 'mc_snowed_dirt', base: 'order_dirt.dds', slot: 'ground9', label: 'Snowed dirt', steps: [
  { op: 'tint', color: [150, 165, 200], amount: 0.25 },
  { op: 'overlay', mask: 'noise', threshold: 0.5, softness: 0.14, scale: 5, texture: 'snow_floor_classic.png', amount: 0.85 },
  { op: 'overlay', mask: 'bright', threshold: 0.36, softness: 0.08, color: [236, 242, 252], amount: 0.6 },
  { op: 'speckle', color: [245, 248, 255], density: 0.04, amount: 0.7 } ] });
t('make_texture', { name: 'mc_burned_grass', base: 'grass_tx_cm.dds', slot: 'ground10', label: 'Burned grass', steps: [
  { op: 'desaturate', amount: 0.55 },
  { op: 'tint', color: [150, 105, 55], amount: 0.45 },
  { op: 'brightness', factor: 0.95 },
  { op: 'overlay', mask: 'noise', threshold: 0.6, softness: 0.15, scale: 5, texture: 'Chaos_dirt.dds', amount: 0.55 },
  { op: 'speckle', color: [70, 65, 62], density: 0.06, amount: 0.8 },
  { op: 'speckle', color: [255, 120, 40], density: 0.006, amount: 0.9 } ] });
t('make_texture', { name: 'mc_frost_road', base: 'gravel_stone_TX_CM.dds', slot: 'ground13', label: 'Frosted road', steps: [
  { op: 'tint', color: [180, 200, 240], amount: 0.3 },
  { op: 'overlay', mask: 'dark', threshold: 0.2, softness: 0.06, color: [235, 242, 252], amount: 0.55 },
  { op: 'overlay', mask: 'noise', threshold: 0.7, softness: 0.1, scale: 4, texture: 'snow_floor_classic.png', amount: 0.55 } ] });
t('make_texture', { name: 'mc_scorched_road', base: 'gravel_stone_TX_CM.dds', slot: 'ground14', label: 'Scorched road', steps: [
  { op: 'tint', color: [130, 85, 65], amount: 0.45 },
  { op: 'brightness', factor: 0.78 },
  { op: 'speckle', color: [60, 55, 52], density: 0.05, amount: 0.8 },
  { op: 'cracks', color: [255, 120, 35], scale: 4, width: 0.012, amount: 0.75 } ] });
t('set_texture', { slot: 'ground11', texture: 'terrain_shared_mine_i_TX_CM.dds', label: 'Mine ground' });
t('set_texture', { slot: 'ground12', texture: 'terrain_shared_periphery_f_TX_CM.dds', label: 'Rocky rim' });
t('set_texture', { slot: 'cliff', texture: 'rocks.dds' });
t('set_texture', { slot: 'mound', texture: 'rocks.dds' });
t('set_texture', { slot: 'brush', texture: 'WallOfGrass.dds' });

// The floating lens-shaped platform over the abyss (rock first; everything walkable is carved).
t('paint', { surface: 'wall', shape: 'polygon', points: PS([8, 172], [45, 128], [110, 92], [200, 52], [300, 22], [400, 4], [500, -2], [600, 4], [700, 22], [800, 52], [890, 92], [955, 128], [992, 172], [970, 218], [905, 262], [830, 312], [760, 350], [660, 384], [560, 402], [500, 406], [440, 402], [340, 384], [240, 350], [170, 312], [95, 262], [30, 218]) });
// Base tips.
t('paint', { surface: 'ground', shape: 'circle', points: PS([70, 168]), radius: 1250 });
t('paint', { surface: 'ground', shape: 'circle', points: PS([140, 168]), radius: 1350 });

// The frost half: snow over the west (the east keeps volcanic rock), with variation patches.
t('paint_texture', { layer: L.snow, shape: 'polygon', points: [[0, 0], [W / 2 - 250, 0], [W / 2 - 250, H], [0, H]], radius: 500, strength: 1, mirror: false });
t('paint_texture', { layer: L.snow, shape: 'polygon', points: [[W / 2 - 250, 0], [W / 2, 0], [W / 2, H], [W / 2 - 250, H]], radius: 300, strength: 0.45, mirror: false });
const PATCHES = [[240, 120], [330, 230], [420, 330], [380, 90], [210, 300], [450, 250], [300, 60], [180, 220]];
t('paint_texture', { layer: L.snow2, points: PATCHES.map(q => P(...q)), radius: 800, strength: 0.55, mirror: false });
t('paint_texture', { layer: L.burned, points: PATCHES.map(q => MX(P(...q))), radius: 800, strength: 0.6, mirror: false });

// Lanes: top and bottom along the rim, mid straight between the nexuses.
t('set_lane', { lane: 'top', width: 720, points: PS([150, 152], [195, 104], [270, 66], [370, 40], [500, 33], [630, 40], [730, 66], [805, 104], [850, 152]) });
t('set_lane', { lane: 'mid', width: 720, points: PS([160, 170], [840, 170]) });
t('set_lane', { lane: 'bot', width: 720, points: PS([150, 188], [190, 258], [270, 330], [370, 370], [500, 378], [630, 370], [730, 330], [810, 258], [850, 188]) });

// The centre spine: top lane - Baron pit - centre - Dragon pit - bottom lane.
t('paint', { surface: 'ground', shape: 'path', width: 420, points: PS([500, 36], [500, 100], [500, 170], [500, 300], [500, 376]), mirror: false });
t('paint', { surface: 'ground', shape: 'circle', points: PS([500, 100]), radius: 720, mirror: false });
t('paint', { surface: 'ground', shape: 'circle', points: PS([500, 300]), radius: 760, mirror: false });
t('paint', { surface: 'ground', shape: 'circle', points: PS([500, 172]), radius: 650, mirror: false });
// Rim platforms beyond the lanes.
t('paint', { surface: 'ground', shape: 'circle', points: PS([500, -14]), radius: 520, mirror: false });
t('paint', { surface: 'ground', shape: 'path', width: 360, points: PS([500, 33], [500, -10]), mirror: false });
t('paint', { surface: 'ground', shape: 'circle', points: PS([500, 424]), radius: 560, mirror: false });
t('paint', { surface: 'ground', shape: 'path', width: 360, points: PS([500, 378], [500, 420]), mirror: false });

// Jungle (west half; the east is mirrored). Paths 440 wide, camps in clearings 900+ off lanes.
// Jungle (west half; the east is mirrored), Summoner's Rift style: every quadrant has two lane
// entrances, a camp route through the buffs and camps, a route to its epic monster pit, a
// teleport pad link, and a link across the mid lane. Paths 560 wide, camp clearings 1300 across.
const J = 560;
const paths = [
  // top quadrant: top-lane entrance -> red buff -> wolves -> wraiths -> Baron pit
  [[228, 88], [258, 108], [285, 122]],
  [[285, 122], [325, 112], [362, 100]],
  [[362, 100], [395, 112], [425, 126]],
  [[425, 126], [465, 112], [500, 100]],
  // top quadrant: mid-lane entrances -> red buff (inner) and -> wraiths (river side)
  [[305, 170], [296, 146], [285, 122]],
  [[440, 170], [432, 148], [425, 126]],
  // top quadrant: teleport pad <-> top lane and <-> wolves
  [[338, 42], [345, 80], [362, 100]],
  // inner route red buff -> wraiths (skips wolves)
  [[285, 122], [330, 140], [385, 142], [425, 126]],
  // bottom quadrant: bot-lane entrance -> golems -> blue buff -> Dragon pit
  [[222, 272], [240, 252], [245, 238]],
  [[245, 238], [290, 258], [335, 276]],
  [[335, 276], [370, 292], [420, 300], [500, 300]],
  // bottom quadrant: mid-lane entrances -> golems and -> wight; wight -> blue buff and Dragon
  [[305, 170], [298, 200], [270, 222], [245, 238]],
  [[440, 170], [425, 200], [408, 228]],
  [[408, 228], [380, 255], [335, 276]],
  [[408, 228], [455, 262], [500, 300]],
  // bottom quadrant: teleport pad <-> bot lane and <-> blue buff
  [[338, 368], [345, 320], [335, 276]],
];
for (const p of paths) t('paint', { surface: 'ground', shape: 'path', width: J, points: PS(...p) });
const CAMP = { wolves: [362, 100], red_buff: [285, 122], wraiths: [425, 126], golems: [245, 238], blue_buff: [335, 276], wight: [408, 228] };
t('paint', { surface: 'ground', shape: 'circle', radius: 650, points: PS(...Object.values(CAMP)) });
const PAD = { top: [345, 80], bot: [345, 320] };
t('paint', { surface: 'ground', shape: 'circle', radius: 380, points: PS(PAD.top, PAD.bot) });
// Pools (same shape both sides): icy water in the west, lava in the east; unwalkable.
const POOLS = [[232, 138], [362, 205], [290, 290]];
t('paint', { surface: 'deep_water', shape: 'circle', radius: 230, points: PS(...POOLS) });

// Bases (blue / west; red mirrored): fountain at the tip, nexus 1800 out, nexus turrets in
// front of it, and on every lane (by distance along the lane from the base): minion spawn,
// inhibitor, inhibitor turret, inner and outer turret.
const laneAt = (pts, d) => {
  for (let i = 0; i + 1 < pts.length; i++) {
    const [a, b] = [pts[i], pts[i + 1]], len = Math.hypot(b[0] - a[0], b[1] - a[1]);
    if (d <= len) return [Math.round(a[0] + (b[0] - a[0]) * d / len), Math.round(a[1] + (b[1] - a[1]) * d / len)];
    d -= len;
  }
  return pts[pts.length - 1];
};
const LANE = {
  top: PS([150, 152], [195, 104], [270, 66], [370, 40], [500, 33]),
  mid: PS([160, 170], [500, 170]),
  bot: PS([150, 188], [190, 258], [270, 330], [370, 370], [500, 378]),
};
// Nexus close to the fountain, as on Summoner's Rift.
// Nexus at Summoner's Rift's distance from the fountain (~1640-1700).
const SPAWN = [900, P(0, 168)[1]], NEXUS = [SPAWN[0] + 1700, SPAWN[1]];
const pl = (kind, p, extra = {}) => t('place', Object.assign({ kind, x: p[0], z: p[1] }, extra));
pl('spawn', SPAWN);
pl('nexus', NEXUS);
pl('turret', [NEXUS[0] + 480, NEXUS[1] + 320], { lane: 'mid', tier: 'nexus_a' });
pl('turret', [NEXUS[0] + 480, NEXUS[1] - 320], { lane: 'mid', tier: 'nexus_b' });
const DIST = { top: [0, 1400, 2100, 4300, 6400], mid: [0, 1250, 1950, 3500, 5200], bot: [0, 1400, 2100, 4300, 6400] };
const STRUCT = [];
for (const lane of ['top', 'mid', 'bot']) {
  const [ms, inh, it, inn, out] = DIST[lane].map(d => laneAt(LANE[lane], d));
  pl('minion_spawn', ms, { lane });
  pl('inhibitor', inh, { lane });
  pl('turret', it, { lane, tier: 'inhibitor' });
  pl('turret', inn, { lane, tier: 'inner' });
  pl('turret', out, { lane, tier: 'outer' });
  STRUCT.push({ kind: 'inhibitor', p: inh }, { kind: 'turret', p: it }, { kind: 'turret', p: inn }, { kind: 'turret', p: out });
}
STRUCT.push({ kind: 'turret', p: [NEXUS[0] + 480, NEXUS[1] + 320] }, { kind: 'turret', p: [NEXUS[0] + 480, NEXUS[1] - 320] });
for (const [k, p] of Object.entries(CAMP)) t('place', Object.assign({ kind: k }, at(...p)));
t('place', Object.assign({ kind: 'baron', team: 'neutral', mirror: false }, at(500, 100)));
t('place', Object.assign({ kind: 'dragon', team: 'neutral', mirror: false }, at(500, 300)));
t('place', Object.assign({ kind: 'health_relic', team: 'neutral', mirror: false }, at(500, -12)));
t('place', Object.assign({ kind: 'health_relic', team: 'neutral', mirror: false }, at(500, 422)));

// No brush: placed by hand later.

// Base wall: a ring of rock around each base, open where the lanes leave (the wall tool keeps
// lanes clear), like Summoner's Rift's base walls.
const RING = [];
for (let k = 0; k <= 24; k++) { const ang = (-105 + k * 210 / 24) * Math.PI / 180; RING.push([Math.round(NEXUS[0] + Math.cos(ang) * 2350), Math.round(NEXUS[1] + Math.sin(ang) * 2350)]); }
t('scenery_wall', { points: RING, keys: ROCKS, size: [260, 380], spacing: 170, width: 300 });

// Walls of scenery: rock formations and spires on every edge; frozen trees fill the west and
// charred trees the same spots in the east.
t('scenery_fill', {
  edge_keys: ROCKS, edge_size: [200, 360], edge_spacing: 150, inset: 0.3,
  fill_keys: FROZEN, fill_size: [420, 640], fill_spacing: 360, fill_chance: 0.8,
  mirror_keys: { [FROZEN[0]]: CHARRED[0] },
});
// Big cliff stones along the outer rim (decor on the wall strip, not blocking more).
t('scenery_wall', { points: PS([120, 74], [210, 36], [300, 12], [400, -4]), keys: CLIFFS, size: [700, 1000], spacing: 520, block: false });
t('scenery_wall', { points: PS([120, 290], [200, 336], [300, 372], [400, 394]), keys: CLIFFS, size: [700, 1000], spacing: 520, block: false });
// Rings of small rocks and crystals around the pools.
for (const [x, y] of POOLS) {
  const ring = [];
  for (let k = 0; k <= 12; k++) { const a = k / 12 * Math.PI * 2; ring.push([x + Math.cos(a) * 13.5, y + Math.sin(a) * 13.5]); }
  t('scenery_wall', { points: PS(...ring), keys: SMALL.concat(CRYSTALS), size: [90, 170], spacing: 110, block: false });
}
// Pits: a crown of crystals around the Baron and Dragon pits.
for (const [x, y, r] of [[500, 100, 33], [500, 300, 35]]) {
  for (const a of [35, 70, 110, 145]) {
    for (const side of [1, -1]) {
      const rad = a * Math.PI / 180;
      t('place_prop', Object.assign({ key: CRYSTALS[0], yaw: a, scale: 2.2, mirror: false }, at(x + side * Math.cos(rad) * r, y + (y < 200 ? -1 : 1) * Math.sin(rad) * r)));
    }
  }
}

// Lanes: runestone decals and stone torches along the edges (mirrored).
const lanes = { top: PS([195, 104], [270, 66], [370, 40], [470, 34]), mid: PS([200, 170], [480, 170]), bot: PS([190, 258], [270, 330], [370, 370], [470, 377]) };
for (const [name, pts] of Object.entries(lanes)) t('prop_row', { key: RUNES[name.length % RUNES.length], points: pts, spacing: 1700 });
const off = (pts, d) => pts.map((p, i) => {
  const a = pts[Math.max(0, i - 1)], b = pts[Math.min(pts.length - 1, i + 1)];
  const dx = b[0] - a[0], dz = b[1] - a[1], l = Math.hypot(dx, dz) || 1;
  return [Math.round(p[0] - dz / l * d), Math.round(p[1] + dx / l * d)];
});
t('prop_row', { key: TORCH, points: off(lanes.mid, 430), spacing: 2100, scale: 0.9 });
t('prop_row', { key: TORCH, points: off(lanes.mid, -430), spacing: 2100, scale: 0.9 });
// Torches around each base platform.
const base = [];
for (let k = 0; k <= 8; k++) { const a = (110 + k * 17.5) * Math.PI / 180; base.push(P(105 + Math.cos(a) * 66, 168 - Math.sin(a) * 66)); }
t('prop_row', { key: TORCH, points: base, spacing: 650, scale: 0.8 });

// Teleport pads: crystals and a glow (blue in the west, pink in the east).
for (const pad of Object.values(PAD)) {
  for (let k = 0; k < 6; k++) {
    const a = k * 60 * Math.PI / 180;
    t('place_prop', Object.assign({ key: CRYSTALS[0], yaw: k * 60, scale: 1.6 }, at(pad[0] + Math.cos(a) * 11, pad[1] + Math.sin(a) * 11)));
  }
}
const lightPair = (p, west, east, radius, intensity, lift = 150) => {
  t('light', { x: p[0], z: p[1], color: west, radius, intensity, lift, mirror: false });
  const m = MX(p);
  t('light', { x: m[0], z: m[1], color: east, radius, intensity, lift, mirror: false });
};
const effectPair = (p, west, east) => {
  t('effect', { name: west, x: p[0], z: p[1], mirror: false });
  const m = MX(p);
  t('effect', { name: east, x: m[0], z: m[1], mirror: false });
};
for (const pad of Object.values(PAD)) {
  lightPair(P(...pad), [90, 170, 255], [255, 90, 170], 520, 1.8);
  effectPair(P(...pad), 'Fire_Cauldron_Blue.troy', 'Fire_Cauldron_Purple.troy');
}
// Pools: mist over the icy water, fire over the lava.
for (const pool of POOLS) {
  lightPair(P(...pool), [120, 180, 255], [255, 110, 40], 700, 1.4, 120);
  effectPair(P(...pool), 'Env_MB_groundMist_01.troy', 'Env_MB_Fire_01.troy');
}
// The halves: snowfall and cold light in the west, smoke and warm light in the east.
for (const p of [[230, 90], [300, 290], [420, 150], [160, 250], [400, 360], [380, 50]]) effectPair(P(...p), 'Env_MB_Snow_01.troy', 'Env_Chimney_Smoke.troy');
lightPair(P(200, 168), [90, 140, 255], [255, 110, 40], 2800, 1.1, 300);
lightPair(P(380, 110), [110, 160, 255], [255, 120, 50], 1800, 0.9, 250);
lightPair(P(380, 260), [110, 160, 255], [255, 120, 50], 1800, 0.9, 250);
t('light', Object.assign({ color: [255, 60, 60], radius: 900, intensity: 1.6, mirror: false }, at(500, 100)));
t('light', Object.assign({ color: [200, 80, 255], radius: 900, intensity: 1.6, mirror: false }, at(500, 300)));
effectPair(lanes.mid[0], 'FireTorch_Blue.troy', 'FireTorch_Med.troy');

// ---- Bases: floor, fountain podium, shop, structure plinths, pillars.
const PG = k => '@Season1-Map3:' + k;
const FOUNTAIN_BASE = PG('tutorial_shrine_base:300-330');
const SHRINE_ORDER = PG('tutorial_stone_steps:235-260'), SHRINE_CHAOS = PG('tutorial_stone_steps:215-234');
const SHOP_STALL = PG('shop_trol_prop:270-290'), KEEPER = PG('shop_trol_character');
const NEXUS_BASE = PG('tutorial_nexus_stonebase.'), INHIB_BASE = PG('tutorial_inhibitor_stonebase_b'), TURRET_BASE = PG('tutorial_turret_stonebase:40-60');
// Visible base floor: Order tiles in the west, the Chaos floor in the east.
const BASE_PTS = [[SPAWN[0] + 200, SPAWN[1]], [NEXUS[0] - 300, NEXUS[1]], [NEXUS[0] + 500, NEXUS[1]], [NEXUS[0] + 300, NEXUS[1] + 700], [NEXUS[0] + 300, NEXUS[1] - 700], [NEXUS[0] - 600, NEXUS[1] + 700], [NEXUS[0] - 600, NEXUS[1] - 700]];
const INHIBS = STRUCT.filter(st => st.kind === 'inhibitor').map(st => st.p);
BASE_PTS.push(...INHIBS);
t('paint_texture', { layer: L.orderFloor, points: BASE_PTS, radius: 900, strength: 1, mirror: false });
t('paint_texture', { layer: L.chaosFloor, points: BASE_PTS.map(MX), radius: 900, strength: 1, mirror: false });
// Lanes leaving the base keep their road.
for (const lane of Object.values(LANE)) t('paint_texture', { layer: L.road, points: [laneAt(lane, 600), laneAt(lane, 900), laneAt(lane, 1200)], radius: 330, strength: 1 });
// Lanes: frosted road on the west half, scorched road on the east half.
const laneDabs = Object.values(LANE).flatMap(pp => { const o = []; const tot = pp.slice(1).reduce((a, q, i) => a + Math.hypot(q[0] - pp[i][0], q[1] - pp[i][1]), 0); for (let d = 0; d <= tot; d += 260) o.push(laneAt(pp, d)); return o; });
t('paint_texture', { layer: L.frostRoad, points: laneDabs, radius: 300, strength: 0.9, mirror: false });
t('paint_texture', { layer: L.scorchedRoad, points: laneDabs.map(MX), radius: 300, strength: 0.9, mirror: false });
// Fountain podium (shrine platform with steps: Order in the west, Chaos in the east) and the shop.
const pair = (westKey, eastKey, p, yaw, scale, lift = 0, walk = false) => {
  t('place_prop', { key: westKey, x: p[0], z: p[1], yaw, scale, lift, mirror: false, walk_through: walk });
  const m = MX(p);
  t('place_prop', { key: eastKey, x: m[0], z: m[1], yaw: 180 - yaw, scale, lift, mirror: false, walk_through: walk });
};
// The fountain exactly as on Summoner's Rift: Map1's Order shrine (blue) and Chaos shrine (red)
// with their stone steps and shop platform, copied from its scene and turned to face the lanes.
t('copy_scenery', { name: 'SR Order fountain', map: 'Map1', center: [104, 287], radius: 700, textures: ['shrine.dds', 'stone_steps.dds', 'base_stone_steps.dds', 'shop_base.dds'], facing: [1, 1], floor: 93 });
t('copy_scenery', { name: 'SR Chaos fountain', map: 'Map1', center: [13998, 14185], radius: 700, textures: ['chaos_shrine.dds', 'stone_steps.dds', 'base_stone_steps.dds', 'shop_base.dds'], facing: [-1, -1], floor: 93 });
pair('asset:SR Order fountain', 'asset:SR Chaos fountain', SPAWN, 0, 1.0, 0, true);
const SHOP = [SPAWN[0] - 120, SPAWN[1] + 300];
pair(SHOP_STALL, SHOP_STALL, [SHOP[0] - 280, SHOP[1] + 330], 200, 1.0);
pair(KEEPER, KEEPER, [SHOP[0] - 150, SHOP[1] + 190], 200, 1.0);
// Plinths under the nexus, inhibitors and turrets.
pair(NEXUS_BASE, NEXUS_BASE, NEXUS, 0, 0.55, 0, true);
for (const st of STRUCT) pair(st.kind === 'inhibitor' ? INHIB_BASE : TURRET_BASE, st.kind === 'inhibitor' ? INHIB_BASE : TURRET_BASE, st.p, 0, st.kind === 'inhibitor' ? 0.45 : 0.4, 0, true);
// Pillars at the base edge (Order pantheon pillars in the west, Chaos shrine pillars in the east).
const PILLARS = [[1500, SPAWN[1] + 1350], [1500, SPAWN[1] - 1350], [2500, SPAWN[1] + 1450], [2500, SPAWN[1] - 1450], [700, SPAWN[1] + 800], [700, SPAWN[1] - 800]];
for (const q of PILLARS) pair('@Map1:orderpantheon:400-450', '@Map1:chaos_shrine:140-170', q, 0, 1.0);
for (const q of PILLARS) lightPair(q, [110, 160, 255], [255, 110, 50], 600, 1.4, 250);
lightPair(SPAWN, [140, 200, 255], [255, 150, 80], 1400, 1.6, 300);
effectPair(SPAWN, 'Fire_Cauldron_Blue.troy', 'Fire_Cauldron.troy');

// ---- Detail texturing.
// Plazas: flagstone in the centre crossing, the pits and on the spine.
t('paint_texture', { layer: L.flagstone, points: PS([500, 172], [500, 100], [500, 300]), radius: 650, strength: 0.9, mirror: false });
t('paint_texture', { layer: L.flagstone, points: PS([500, 60], [500, 135], [500, 235], [500, 340]), radius: 260, strength: 0.8, mirror: false });
// Jungle paths: dirt (Order dirt in the west, Chaos dirt in the east).
const dens = (pts, step) => { const o = []; for (let i = 0; i + 1 < pts.length; i++) { const [a, b] = [pts[i], pts[i + 1]], n = Math.max(1, Math.ceil(Math.hypot(b[0] - a[0], b[1] - a[1]) / step)); for (let k = 0; k < n; k++) o.push([Math.round(a[0] + (b[0] - a[0]) * k / n), Math.round(a[1] + (b[1] - a[1]) * k / n)]); } o.push(pts[pts.length - 1]); return o; };
const pathDabs = paths.flatMap(pp => dens(PS(...pp), 280));
t('paint_texture', { layer: L.orderDirt, points: pathDabs, radius: 240, strength: 0.85, mirror: false });
t('paint_texture', { layer: L.chaosDirt, points: pathDabs.map(MX), radius: 240, strength: 0.85, mirror: false });
// Camp clearings: worn mine ground.
t('paint_texture', { layer: L.mine, points: PS(...Object.values(CAMP)), radius: 430, strength: 0.7 });
// Around the pools: packed snow in the west, cracked lava ground in the east.
t('paint_texture', { layer: L.snow2, points: PS(...POOLS), radius: 520, strength: 0.9, mirror: false });
t('paint_texture', { layer: L.cracked, points: PS(...POOLS).map(MX), radius: 560, strength: 0.9, mirror: false });
// The rim (wall strip by the abyss): rocky ground.
const RIM = PS([45, 128], [110, 92], [200, 52], [300, 22], [400, 4], [500, -2], [400, 402], [340, 384], [240, 350], [170, 312], [95, 262], [30, 218]);
t('paint_texture', { layer: L.rim, points: RIM.flatMap((q, i) => i + 1 < RIM.length && i !== 5 ? dens([q, RIM[i + 1]], 400) : [q]), radius: 520, strength: 0.75 });
// The east edge of the lava sea glows: cracked ground along the volcanic rim.
t('paint_texture', { layer: L.cracked, points: RIM.slice(0, 6).concat(RIM.slice(6)).map(MX), radius: 450, strength: 0.6, mirror: false });

// ---- Height: the whole map is flat.

t('check');
t('finish', { summary: "Magma Chamber (2010 5v5 'Caldera' concept, Concept 04 layout): a lens-shaped platform over the abyss, frost side (west, blue) and volcanic side (east, red) with the same mirrored layout: top and bottom lanes along the rim and a straight mid, 3 turrets per lane, inhibitors, nexus turrets, a north-south spine joining the Baron pit, the centre and the Dragon pit, six camps per side in clearings off the lanes, teleport pads, icy pools in the west and lava pools in the east, health relics on the rim platforms. Walls are built from Season 1 Crystal Scar / Twisted Treeline scenery (rock formations, frozen trees in the west, charred trees in the east, cliff stones on the rim) with runestone decals and stone torches along the lanes." });

fs.writeFileSync(__dirname + '/magma_chamber.json', JSON.stringify(steps, null, 1));
console.log(steps.length + ' steps written');
