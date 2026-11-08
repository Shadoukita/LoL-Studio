# LoL Map Studio

An editor and inspector for League of Legends map folders (`LEVELS/MapN`), written in Rust. It
targets the 4.17-era client and reads and writes both kinds of map:

- **Legacy maps** like `Map1`: `Scene/room.nvr` + `room.mat`, turrets and inhibitors as
  `.SCB`/`.sco` meshes listed in `room.dsc`.
- **Baked maps** like `map11`: `Scene/room.wgeo` with `terrain.inibin` → `Renderer = baked`,
  structures as entries in `MapObjects.mob`.

## Game files

No game files are included. Put your own `DATA` and `LEVELS` folders next to the program (`lol-map-studio.exe`), or into the `game` folder (see `game/README.md`):

```
game/
  DATA/     the game's DATA folder (characters, particles, HUD)
  LEVELS/   map folders: Map1, map11, ..., optional collections such as Season1/Map8
```

The editor and `lmscli` find `game/` next to the program or the working folder (or above
them); `LMS_GAME_DIR` points them at another folder holding `DATA` and `LEVELS`. Both folders
are ignored by git. Tests that need game files skip themselves when they are missing.

## Build and run

```powershell
cargo build --release
.\target\release\lol-map-studio.exe                      # start screen
.\target\release\lol-map-studio.exe game\LEVELS\Map1  # open a map directly
```

## What it does

**Open and inspect.** *File → Open map folder* (or *Open example*). The 2D view shows the scene
rendered from above, with turrets, inhibitors, nexuses, spawns, minion barracks and lane points.
The 3D tab shows the scene geometry. The left panel lists objects and files. The right panel edits
whatever is selected:

| File | Editor |
|---|---|
| `MapObjects.mob` | every field of each object (name, type, position, rotation, scale, box, skin) |
| `.SCB` / `.sco` objects | position (both files move together), their `ObjectCFG.cfg` section |
| `*.inibin`, `*.cfgbin` | value table with names for known keys; edit, add, remove |
| `room.nvr` | materials (name, type, flags, textures), meshes (hide, move), OBJ import |
| `room.wgeo` | mesh textures and materials, OBJ import (optionally walkable) |
| `AIPath.aimesh_ngrid` | statistics; edit by painting on the map |
| `.dds` | preview; replace from PNG/JPG/DDS |
| text files (`.dat`, `.cfg`, `.ini`, `room.mat`, `.dsc`, …) | text editor |
| `.luaobj` | the string constants the script uses (scripts are copied unchanged) |

**Edit and add.**
- *Select / move*: drag objects.
- *Paint grid*: walls, walkable ground, brush, and raise/lower/flatten/smooth heights.
- *Place object*: new turrets, inhibitors, spawns, nav points, props and so on.
- *Duplicate* (Ctrl+D): copies the selected structure, including its mesh files and its
  `ObjectCFG.cfg` stats.
- *Map → Import OBJ model* adds custom geometry to the scene.
- *File → Import file* adds files; PNG/JPG images become DDS textures.

Ctrl+Z / Ctrl+Y undo and redo.

**Create new maps.** *File → New map…* generates a complete map folder:
- the scene (`room.nvr` or `room.wgeo`), terrain textures, navigation grid with pathfinding hints,
  AI mesh, map objects, `room.dsc`, and the minimap;
- terrain at any size, either from the template's lanes and jungle (scaled) or as an open field.

Gameplay data (level scripts, `*.inibin` settings, structure meshes) comes from a template map.
The level scripts address turrets, inhibitors and spawns by name, so the template's structures are
placed proportionally on the new map. The scene format follows the template (Map1 → legacy,
map11 → baked).

On a generated map, the navigation grid is the terrain layout. Painting the grid and choosing
*Rebuild terrain* (or exporting) regenerates the scene geometry: walls become cliffs, brush gets
grass.

**Export.** *File → Export map…* (Ctrl+E) writes the whole folder to `<LEVELS>\MapN`; the folder
name selects the map ID.
- Files you did not change are written **byte for byte as they were**. Changed files are
  re-encoded in the game's own formats.
- The map is assembled in a temporary folder first, so a failed export leaves the old folder intact.
- *Map → Check map* lists problems before you export: a scene format that doesn't match
  `terrain.inibin`, missing spawns, objects off the grid, missing textures, and names the scripts
  expect.

## Custom maps from scratch

*New custom map…* opens a painting canvas:

- **Surfaces**: ground, wall/cliff, brush, river (walkable water), deep water, empty. Paint them
  with the brush, rectangle, ellipse, polygon or fill tools.
- **Ground textures**: four layers mixed by painting (grass, dirt, stone path and sand by default;
  any example-map texture or your own image can be used).
- **Sculpt**: raise, lower, flatten or smooth the ground.
- **Objectives**: nexus, inhibitors, turrets (outer, inner, inhibitor and nexus tiers), fountains
  (spawn, shrine turret and shop) and minion spawns, each with a team and a lane.
- **Lanes**: draw each minion lane from the blue base to the red base. *Place lane structures* adds
  the standard turrets, inhibitor and minion spawn per lane.
- **Symmetry**: diagonal (like Summoner's Rift), rotational or mirrored. Painting and objectives are
  copied to the other team's side.
- **Preview (3D)** shows the built map with the game's shading and the real structure models.
  *Game look* renders the 2D canvas the same way.

Projects save as `.lmsmap` files. *Build and export* writes `LEVELS/Map<ID>`:
- the scene: four-blend ground with a painted mask like Summoner's Rift, rock cliffs, grass and a
  Lake01 water object;
- the navigation grid with pathfinding hints, AI mesh, map objects and ObjectCFG stats;
- the turret script `CreateLevelProps.lua`, lane nav points, the sun settings in `terrain.inibin`
  and the minimap.

Generated scripts are plain Lua text, which the client loads like compiled `.luaobj`. **Rules**:
- **Classic** copies the generic gameplay files (items, XP, minion waves) from a rules map
  (Map1 by default). That map's terrain, objects and jungle are not used.
- **Sandbox** writes only generated scripts: structures and spawns, no minion waves. It is
  experimental.

## Scenery, ground layers, structures

- **Ground layers**: as many as 16. Add any library texture or your own .dds/.png/.jpg (Texture tool or Look). Each area of the map mixes its four strongest layers in its own four-blend material.
- **Scenery** (G): about 500 props extracted from Map1 (trees, rocks, plants, lights, statues, props), shown with thumbnails by category. Place, scatter with a brush, or erase. Select props to move, turn, resize or raise them. Import your own OBJ model with a texture under My props. Large trees, rocks and statues block walking.
- **Jungle walls** (League style, the default for new maps): as on Summoner's Rift, walls are not raised terrain but 3D models on flat ground: Map1's big rocks line every wall edge (sized to the wall, not overlapping) and tree canopy fills the inside; the navigation grid marks the wall as unwalkable. Rock spacing, your own edge rocks, tree choice and spacing are under Jungle walls. Default style **Classic: trees / mound / trees**, as on classic Summoner's Rift (Map1): pine trees ring every wall along its outline, the middle is a low mossy mound (Map1's foliage_sr_leaves texture; height and texture adjustable) with a few trees on top. **Rocky edges** puts stone faces on the edges instead; the Edge: rocks ↔ trees slider mixes the two. **Auto-dress**: every painted wall is traced into a smooth outline; rocks (and a share of small trees) are placed by walking that outline at an exact step, turned along it and set a little inside, and trees fill the inside on a jittered grid kept off the edge. Paint, erase or reshape a wall and the dressing follows; extending a wall keeps the rest of it as it was. Settings: rock spacing, rock size range, how far inside the edge, trees on the edge, follow the outline. **Bake into props** turns the dressing into ordinary props to move or delete one by one. On opened maps (Map1 ...) only walls you paint get rocks and trees; the map's own walls are left as they are. "Raised terrain" keeps the older extruded cliffs.
- **Structures**: spawn point, fountain turret and shop are separate objectives; the Fountain preset places all three.
- **Jungle camps**: Blue buff, Red buff, Wolves, Wraiths, Golems, Wight, Dragon and Baron (Objectives tool), or "Add Summoner's Rift jungle" for all 14. Export writes Scripts/NeutralMinionSpawn.lua with the tables and functions of Riot's script (rebuilt from Map1's bytecode), using Map1's formations, respawn times, delays, timers and minimap icons. The preview and Play show the monsters with their DATA models.
- **Base pieces and decals**: the Scenery library also holds Summoner's Rift's base walls, gates, Chaos buildings, shrine platforms, steps, jungle camp bases and ground decals (roads, chasms, stone bases, flowers). Walls block walking under their footprint.
- **More tools**: Line (L) for walls and paths, [ and ] to resize brushes, a hotkey for every tool, show or hide objectives, lanes and props.

## Play mode (F5)

Play walks the map inside the editor as Katarina (DATA/Characters/KatarinaV2), with no game server, and follows the 4.17 client:
- **Camera**: 56 degree pitch, 50 degree field of view (40 on map 11, with the client's zoom correction), zoom distance 1000 to 2250 starting at the maximum, 120 per wheel notch, eased over 0.2 s. It looks at ground level, starts free (Game.cfg LockCamera=0), and edge, arrow or middle-mouse scrolls at about 1111 units/s with the client's ramp. Y toggles the lock and Space centres the camera while held.
- **HUD**: the game's own, drawn from DATA/menu/hud (ElementsListNormal.ini, Scenes.ini and the HUDAtlas). Element rectangles use the client's anchor/aspect scaling. Katarina's portrait, spell, passive and summoner icons and level-1 stats fill it in, along with the top bar (kills, CS, game time, FPS) and the minimap frame with camp icons and the camera outline.
- **Movement**: right-click to move, with A* on the navigation grid; walls, structures and jungle monsters block, and brush makes her see-through. S stops, Q/W/E/R play spell animations, D flashes, B recalls, Ctrl+1-4 emotes, the minimap moves the camera (left) or Katarina (right), Esc leaves.

## Edit (3D)

Every tool also works directly in the 3D view (the **Edit (3D)** tab), on the terrain under the cursor:
- **Camera** (Roblox Studio style): right-drag orbits, middle-drag pans, wheel zooms, WASD moves (Shift: faster), F focuses the selection, Home shows the whole map.
- **Painting**: brush, texture and sculpt strokes, line/rectangle/ellipse/polygon/fill and lanes, with the brush ring lying on the ground (plus its mirrored copy). Ctrl+wheel resizes the brush. The terrain rebuilds continuously while you paint.
- **Transform gizmo**: the selection shows a gizmo at its centre. Drag the red / blue arrows to move along X / Z only, the green arrow to raise or lower (props, effects, lights), the gold ring to turn (a group turns around its centre). Grid snap and the angle step (or Shift) apply.
- **Scenery from** (Scenery tool, left panel): which maps' art a map uses. New maps use Map1 (classic Summoner's Rift) only, so an old-style map never gets modern scenery; opened maps use their own folder. It limits the scenery list, scatter, AI decoration, the default wall rocks and trees, and the Look texture lists. Your own models are always available.
- **Drawing rows** (Scenery → Path): Freehand follows the stroke you draw (smoothed), placing the chosen prop at an exact spacing and turned along the line: tree lines, hedges, fences, torches. Straight draws a straight row.
- **Placement previews**: objectives (the real structure and monster models) and scenery appear as see-through ghosts at the cursor, with their mirrored copy. Press, drag and release to turn them toward the drag (Shift snaps to the angle step); R / Shift+R turn by the step; Ctrl+wheel sizes the next prop. The 2D canvas shows the footprint and facing arrow and turns by dragging too.
- **Select**: click to select, drag to move, Shift+drag to turn, R to turn, Ctrl+D to duplicate, Del to delete; the selection is highlighted at its live position.
- **Snapping**: grid (25 to 500 units) and angle step, on the toolbar.

## Editing existing maps (Map1, map11, your exports)

"Edit Map1 with the map tools" (start screen), "Edit with map tools" (inspector toolbar) or File > "Edit a map folder" opens a map with all the editor's tools: Edit (3D), Paint (2D), ghosts, scenery, effects, lights, camps, Dominion/ARAM objects and Play.
- The map is read into an editable project: walls and brush from its navigation grid, structures with their original names and skins (turret tiers and lanes read from its compiled CreateLevelProps, including map11's SpawnTurret calls), lanes from its nav points, jungle camps, Particles.dat effects and Light_Env.dat lights.
- The map's terrain, scenery and textures are kept. Build and export starts from the original folder and rewrites only what changed: an unchanged map exports byte for byte (verified on Map1 and map11). Changed structures replace their map objects, scene meshes, ObjectCFG sections and turret script; changed lanes, camps, effects and lights rewrite their files; painted walls and brush update the navigation grid; new props and painted walls are added to room.nvr (or room.wgeo on baked maps).
- In the views, the original scene is drawn once; structures, new props, monsters, effects and painted cells (brown new wall, green new brush, yellow newly walkable) are drawn live.
- Limits: existing scenery art cannot be removed or moved (it is baked into the scene); a compiled jungle script whose camps cannot be read (map11) is kept as it is.

## Tool bar, outliner, selection

- **Tool bar**: a vertical icon strip next to the viewport (Select, Brush, Line, Rectangle, Ellipse, Polygon, Fill, Texture, Sculpt, Objectives, Lanes, Scenery, Effects, Undo, Redo) with keys and help on hover, and an options bar above the viewport with the active tool's settings, like Blender and Photoshop.
- **Outliner** (left panel): every structure, camp, game-mode object, prop (by category), effect, light and lane, searchable. Click selects, Ctrl+click adds, double-click looks at it.
- **Selection** in Edit (3D): Ctrl+click adds, Shift+drag on the ground box-selects, dragging moves the whole group, Ctrl+C / X / V copy, cut and paste at the cursor, Ctrl+D duplicates, Del deletes.
- **Scenery Path mode**: drag a line to place a row of a prop (fences, torches, walls, tree lines) turned along it. **Follow slope** tilts placed props to the ground.

## Create a map with AI

"Create a map with AI" (start screen, Map menu, or the AI map button) turns a description into a map. With an Anthropic API key (field in the dialog, or the ANTHROPIC_API_KEY environment variable) Claude designs it (Opus 5.5 by default; Sonnet 5.5 or Haiku 4.5 are faster); without a key a built-in reader understands common words (lane counts, ARAM, Dominion, snow / desert / night / volcanic / autumn / Shadow Isles, river, jungle, walls, brush, hills, torches, flowers, rocks, ruins). The plan is shown first; "Build this map" creates a normal editable map: lanes and structures, generated terrain and river, camps, Dragon and Baron, game-mode objects, decoration from the scenery library, effects, sun and ambient, fog and picture settings. The key is stored only in the local settings file; the prompt is sent to Anthropic.

**The AI builds the map itself ("Build it step by step").** With a Claude key, Claude works the editor's tools one call at a time, like a designer: new map and symmetry, terrain (walls, paths, river, lava, void), every lane, the nexus, spawn, each turret with its lane and tier, inhibitors, minion spawns, camps, Baron / Dragon, brush, textures (Map1's, or fetch_texture: CC0 ground textures from Poly Haven restyled like Map1), scenery (library props, or fetch_model), effects, lights, fog and colour. It looks at the map through an overview (a character picture with every object) and a playability check (bases connected, walking distance per team, lanes blocked or narrow, structures per team, turret spacing along each lane, jungle chokepoints, brush, camps) and fixes what the check reports, then finishes. The map updates live in Edit (3D); Stop ends it early; everything stays editable. Attach concept art as a **reference image** and Claude follows its layout. Requests use Claude Opus 5.5 at high effort with prompt caching and Anthropic's server-side refusal fallback (another model finishes a request if one declines).

**Magma Chamber recipe.** The AI dialog's "Magma Chamber (2010 5v5 concept)" builds Riot's unreleased 5v5 Magma Chamber ("Caldera") offline with the same tools, following Riot's 2010 concept art (Concept 04), fully mirrored left / right: a 21000 x 10500 lens-shaped platform over the abyss, a frost side (west, blue: snow ground, frozen pines, icy water) and a volcanic side (east, red: volcanic rock, charred trees, lava) with the same layout; top and bottom lanes along the rim and a straight mid, 3 turrets per lane, inhibitors, nexus turrets, minion spawns; a north-south spine joining the Baron pit, the centre and the Dragon pit; six camps per side in 1000-wide clearings 900-1600 units off the lanes on 440-wide paths; teleport pads (marker crystals and glow; 4.17 has no teleport object without scripting); pools ringed with rocks and crystals; health relics on the rim platforms. Walls are built from scenery (Season 1 Crystal Scar and Twisted Treeline models, Map1 rock spires), not the wall brush; runestone decals and stone torches line the lanes; snow, mist, smoke and fire effects and per-side lights. recipes/magma_chamber_gen.cjs generates it; `lmscli agentscript recipes/magma_chamber.json <out> [project]` runs it, prints the overview and the playability check and builds the map.

Bases and texturing (Magma Chamber): each side has a proper base at its tip: the fountain on Proving Grounds' shrine podium with stone steps (Order steps in the west, Chaos in the east), the shopkeeper and his stall, the nexus on its stone plinth with two nexus turrets in front, and per lane a minion spawn, the inhibitor (1250-1350 along the lane) and the inhibitor, inner and outer turrets (about 2000 / 4300 / 6400 along the lane), each on a stone plinth; the base floor is painted with Order tiles (west) and the Chaos floor (east), with Order pantheon / Chaos shrine pillars and lights around it. The ground uses 13 texture layers: snow and packed snow (west), volcanic rock and burned ground (east), stone road on the lanes, Order / Chaos base floors, flagstone plazas at the centre and the pits, Order / Chaos dirt on the jungle paths, worn mine ground in the camps, packed snow / cracked lava ground around the pools, rocky ground along the rim. Own textures (make_texture, an AI tool: tint, desaturate, brightness, overlay another texture through a bright / dark / noise mask, glowing cracks, speckles; seamless): Magma Chamber uses snowed dirt and burned grass on the jungle paths, a frosted road and a scorched road on the lanes per half, a frost Order floor with snow in the tile cracks and an ember Chaos floor with lava veins. The nexus stands 1550 from the fountain (Summoner's Rift: about 1640; `lmscli basedist <map>` measures a map's base distances). The AI's set_texture adds ground layers as needed (up to 16) and searches every example map's textures (list_textures). `lmscli agentscript` also checks the exported navigation grid: spawns walkable, bases connected, no structure on a blocked cell.

Lanes stay clear: the scenery tools leave out any model whose footprint would reach into a lane (lane centre + 380 + 0.85 x its radius) and place_prop warns; `lmscli laneprops <project>` lists tall models near lanes. Magma Chamber has no brush (place it by hand). The whole map is flat. The fountains are Summoner's Rift's own: the Order shrine (blue) and Chaos shrine (red) with their stone steps and shop platform, copied mesh by mesh from Map1's scene (copy_scenery, also an AI tool; `lmscli nvrnear <map> x z r` lists a map's scene meshes near a point) and turned to face the lanes. They and the structure plinths are walk-through decoration (no collision), so champions walk onto the fountain freely, and they stand on top of it: the upward-facing tops of walk-through props (up to 180 above the ground) are written into the navigation grid heights the game and Play use for units. Any prop can be made walk-through in its properties (Collision: Walk through) or by the AI (place_prop walk_through). The nexus is 1700 from the fountain, inhibitors ~2000 from the nexus, inhibitor turrets ~2700, inner ~4300-4900, outer ~6000-6800, as on Summoner's Rift; a rock base wall (radius 2350 around the nexus) encloses each base, open only where the three lanes leave (scenery walls open automatically at lanes). The jungle has 560-wide paths laid out per quadrant: two lane entrances, the camp route (red buff, wolves, wraiths / golems, blue buff), routes to the Baron and Dragon pits, mid-lane crossings, an inner shortcut and the teleport pad links; camp clearings are 1300 across. Prop names in the scenery library ("lambert #12") are now the same on every load, so saved projects keep pointing at the same models.

**Scenery tools for the AI.** scenery_wall (a wall made of models along a line, the ground under it unwalkable) and scenery_fill (every wall area dressed with chosen models as real props, edges and inside, optionally another model for the mirrored half: same positions, different art per side). Model selectors "@Map:texture[:minHeight-maxHeight]" (e.g. "@Season1-Map8:frozen_props_tree_02:350-1200") pick models by texture, since names like "lambert #12" depend on what is loaded. The playability check also checks every camp: on a lane (PROBLEM under 500 units, warning under 900), reachable from its base, reachable through passages 240+ wide, clearing size.

**Season 1 maps.** game/LEVELS/Season1/MapN (Season 1 Summoner's Rift, Harrowing Rift, Proving Grounds, Twisted Treeline, Crystal Scar) join the scenery library as "Season1-MapN" (Scenery from); older .tga texture references are resolved to the .dds files.

**Lava.** Look: "lava" turns deep water into a glowing lava surface (unwalkable) instead of the water plane.

**Online models in the classic style.** The plan can ask for objects the classic scenery does not have (Claude picks them; the offline reader understands barrels, crates, carts, lanterns, statues, boulders, stumps, logs, fences, benches, wells, pots, ruins). "Build this map" then searches Poly Haven (free CC0 models, polyhaven.com; no account or key), skips modern things, downloads the best two matches per object (1k glTF) and restyles them like classic 2014 Summoner's Rift (Map1) art: cut to a classic polygon budget (about 1000 triangles, trees 1800), scaled to League size (kept near what that kind of object measures on Map1), textures shrunk to 256 px, softened, posterized and graded toward a Map1 texture of the same kind. They are saved to your assets (Model Studio, category as chosen) and dotted over the map; edit or delete them like any asset. Downloads are cached in %APPDATA%/LoL Map Studio/online. Summoner's Rift and night themes now use Map1's own ground, cliff and brush textures. Test from the command line: `lmscli onlinetest "wooden barrel"`, `lmscli aitest "<prompt>"`.

Copy and paste now work in every text field (the clipboard was not built in before).

The 3D views now show the map's picture settings (gamma, brightness, contrast) and height fog, and the app has a new dark theme with gold and blue accents.

## World-building tools (inspired by Roblox Studio, Unreal and the StarCraft II editor)

- **Generate terrain** (Sculpt tool): jungle walls, brush, hills and a river from a seed, symmetric, with lanes and objectives kept clear. Undo restores the old map.
- **Heightmap import / export**: any image (bright = high, optional walls above a brightness), or save the map's heights as a PNG.
- **Sculpt Noise** (bumps) and **Ramp** (drag from the low end to the high end).
- **Pathing view** (tool bar): red where units cannot walk, green brush.
- **Measure tool (M)**: distance and walking time; turret attack, fountain turret and fountain healing ranges on the map.
- **Selection actions**: align (min / centre / max on X or Z), spread evenly, randomize turn and size, drop to ground, mirror to the other side, replace model, lock / unlock, delete.
- **Prefabs**: save any selection as a reusable group and place it on any map (Scenery tool); it follows the cursor with previews, Shift+click places it again.
- **Camera bookmarks**: Ctrl+1..9 saves a view in Edit (3D), 1..9 returns to it.

## Look & behaviour

The right panel's **Look & behaviour** tab edits the per-map settings the game reads, with the map's own values shown by default: height fog (colour, opacity, heights), fog of war (brightness, fade, edges), cloud shadows (texture, size, speed), picture (level, gamma, brightness, contrast, texture sharpness, leaf cut-off), shadows (colour, softness, bias, fade), brush grass (see-through, sway), theme music and ambience, atmosphere variant, camera bounds and draw distance. Gameplay settings for a server: starting and passive gold, fountain regen, experience and gold ranges, respawn scaling and per-level respawn times, the experience curve. They are written into terrain.inibin, clouds.inibin, Gamma.inibin, ShadowSettings.inibin, Graphics.inibin, Grass.ini, Audio.inibin, AtmosphereMutators.inibin, Constants.var, DeathTimes.inibin and ExpCurve.inibin, keeping each value's stored type.

## Model Studio (custom 3D assets)

"Model Studio" (start screen, or the top bar while editing a map) is a Blender-style editor for your own map assets:
- **Import any 3D model**: OBJ (with materials), glTF/GLB, STL, PLY and League's SKN, SCB and SCO. Or open a Summoner's Rift prop, a game character, an asset from your library, or start from shapes (cube, cylinder, cone, sphere, plane, torus, ramp, rock). Imported models are mirrored into League's coordinates and stood on the ground.
- **Object mode** works on whole parts and **Edit mode** (Tab) on vertices. G / R / S move, rotate and scale, X / Y / Z lock an axis, Ctrl snaps, click or Enter confirms, right-click or Esc cancels. Click, Shift+click and drag-box selection; A / Alt+A select all or none; Shift+D duplicate, X delete, E extrude, L select linked, P separate, Ctrl+J join, H / Alt+H hide or show; numeric move/rotate/scale and mirror.
- **Mesh tools**: smooth / auto-smooth / flat normals, flip faces, merge by distance, subdivide, decimate, box and top-down UVs, separate loose parts.
- **Materials**: load a texture per part (PNG, JPG, TGA, BMP, DDS), share textures between parts, tint (baked into the game texture).
- **Library**: Save keeps the asset in your library (%APPDATA%/LoL Map Studio/assets) with game-ready DDS textures; it appears under "My assets" in the Scenery tool. "Use in map" puts it on the cursor in Edit (3D). "Blocks walking" makes units walk around it. Export as OBJ for other tools. Undo and redo for every edit.

On maps, every placed prop now has a full transform: position, height, turn, size, tilt (X and Z) and per-axis stretch (properties panel); in Edit (3D), Ctrl+wheel resizes and Alt+wheel raises the selected prop.

## Effects, lights, game modes, animated monsters

- **Effects tool (X)**: any of the 5800 particle effects in DATA/Particles (.troybin), and point lights. They are written to Particles.dat, Light_Env.dat (version 3) and Light.dat in the client's formats. Effects play live in Edit (3D), Preview, Play and the inspector's 3D view (Summoner's Rift's own 623 effects included).
- **Dominion and ARAM objects** (Objectives tool): health relics (written as ARAM-style camps), capture points, speed shrines, center relics and Dominion nexus crystals (written as Map8-style Info_* markers). Their characters are preloaded. The game server spawns them, as in the real game modes.
- **Animated monsters**: jungle monsters play their idle animations in every 3D view and in Play mode; relics and shrines show their particle auras.

## Game-accurate rendering

The 2D and 3D views draw what the game draws:
- Shading follows the client's own HLSL (`Shaders/HLSL/Environment`): lit materials, four-blend
  ground, decals, wall of grass and baked WGEO.
- The sun and ambient colours come from `terrain.inibin [Sun]`.
- With the game's `DATA` folder in `game/DATA`, turrets, inhibitors and
  nexuses are drawn with their character models. Textures that `room.mat` takes from DATA are found
  as well.

## Verification

`lmscli` (built alongside) runs the checks used during development:

```powershell
.\target\release\lmscli.exe roundtrip game\LEVELS\Map1 game\LEVELS\map11
.\target\release\lmscli.exe edittest  game\LEVELS\Map1 C:\Temp\Map1-edit
.\target\release\lmscli.exe new       game\LEVELS\Map1 C:\Temp\Map30 12000 [--blank]
.\target\release\lmscli.exe validate  C:\Temp\Map30
```

- `roundtrip` re-encodes every parsed file through its writer. All 643 files of Map1 and map11
  come out identical.
- `edittest` edits a map (moves, duplicates, places, paints, changes a setting, imports a model),
  exports it, reopens it, and checks that every untouched file is identical and every edit landed.

## Formats

| File | Notes |
|---|---|
| `room.nvr` | v9.1; vertex size by material type and flags |
| `room.wgeo` | v3–5, with bucketed triangle lists |
| `AIPath.aimesh_ngrid` | v2–5, 56-byte cells, 900 hint nodes |
| `MapObjects.mob` | "OPAM" v2, 128-byte records |
| `.SCB` / `.sco` | the game prefers `.SCB` |
| renderer choice | `terrain.ini [Environment] Renderer` = `legacy` / `baked` |
| world objects | `MapObjects.mob` first, then `room.dsc`; class from the name prefix |

## Limits

- Generated and edited maps have been checked against the loaders' formats, not played in the
  game. Play mode simulates walking only: no combat, minions or abilities beyond their animations. Jungle monsters stand in their idle pose. The client spawns camps through the game server, so they appear in a real game only when the server runs the map's NeutralMinionSpawn script.
- Rebuilt WGEO ground buckets match map11's in triangles, vertices and indices; their "out-stick"
  margins can differ in the last digits.
- Level scripts (`.luaobj`) are compiled Lua and are copied, not edited. New structure names need
  script support to take part in game logic (Duplicate copies their stats).
- `Scene/AIPath.*` meshes are not generated; the client skips objects named AIPath.
- For a generated map, only its navigation grid is kept as the terrain layout. Reopening it detects
  its generated materials; the cliff height falls back to the default.
