# Game files

LoL Map Studio does not include any game files. Copy your own here:

```
game/
  DATA/     the game's DATA folder: Characters, Particles, menu/hud, Spells, ...
  LEVELS/   map folders, e.g. Map1, map11; collections such as Season1/Map8 also work
```

- `LEVELS/Map1` (classic Summoner's Rift) is the main source of scenery, textures and rules for
  new maps; other maps add more scenery (they show up as "Season1-Map8" and so on).
- `DATA` is needed for Play (the champion, HUD and abilities), structure and monster models,
  and particle effects.

Everything you put in `DATA` and `LEVELS` is ignored by git. To keep the files somewhere else,
set the `LMS_GAME_DIR` environment variable to a folder holding `DATA` and `LEVELS`.
