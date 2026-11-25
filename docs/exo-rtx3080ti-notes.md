# victory-exo RTX 3080 Ti Notes & Performance Log

Hardware: **Windows 11, NVIDIA RTX 3080 Ti (12 GB VRAM, SM 6.6, Wave32), 64 GB RAM** (Series X profile).

## Build `exo-0b0074e` (2026-11-25)

### What Worked (Verified Live)
- **Water physics & boat buoyancy (`Geckomacabre/vx-layer@eee5667`, `VICTORY_TEXTURE_BACK` on by default):**
  - Copies GPU-written CPU-read ocean height/displacement textures (`256x256` `R32_FLOAT` / `R32G32_FLOAT`, `64x64` `R32G32_FLOAT`, `128x128` `R16G16_UINT`) back into title memory in console-tiled layout.
  - Water and boat interaction are fixed.
- **Facial & mouth animations (`fxceless/victory-fork#927`):**
  - Preserving 16-bit buffer view bits through unsigned integer views fixed facial animations.
  - Minor remaining visual note: slight artifact on the lower-left edge of one character's mouth during a cutscene.
- **Wave32 shader coverage (`fxceless/victory-fork#993`, includes `#854`) & pipeline fixes (`vx-layer` `cf815a8`, `5492b73`, `b2d82ab`):**
  - **0 shader translation failures**, **0 pipeline refusals**, **0 errors** across the full session.
  - All 612 release unit tests pass (`victory-formats` 29/29, `victory-hle` 120/120, `victory-gpu` 196/196, `victory-shader` 267/267).
- **Color & material fixes (`#374`, `#764`, `#765`, `#911`):**
  - BC7 rainbow-block artifact fixed; 64 KiB CBV clamp keeps NVIDIA device stable with zero device loss.

### Launcher Switches Used
```bat
VICTORY_ASYNC_SUBMIT=1
VICTORY_QUEUE2=1
VICTORY_PLACED_TEX=1
VICTORY_LEVELS_ON_DEMAND=1
VICTORY_EARLY_TEXTURES_MS=120
VICTORY_WIDE_TEXTURES=1
VICTORY_INDIRECT_WINDOW=1
VICTORY_CARD_REHASH_RUNS=1
VICTORY_CARD_BUFFERS_MIN_MB=1
VICTORY_CARD_BUFFERS_MB=1024
VICTORY_TEXTURE_NO_WAIT=1
VICTORY_PIPELINE_WAIT_MS=24
VICTORY_JOIN_COPIES=1
```

### Measured Performance Breakdown (Steady-State World, 0 Pipelines Compiling)
| Metric | Measured Value | Notes |
|---|---|---|
| Menus / Pause | **30.0 fps** (33.3 ms/frame) | 135 draws/frame, 0 dropped |
| Open world (steady state) | **3.4–3.6 fps** (281–295 ms/frame) | ~3,850–3,980 draws/frame, 0 indirect dropped |
| **GPU time** | **170–179 ms/frame (~61%)** | **#1 bottleneck**: per-draw indirect clamps/barriers + 17,839 card-compare pieces |
| **Settle waits** | **184.5 ms/frame** | `88.4 ms` title memory changed under shader writes + `96.1 ms` title memory read |
| **Preparing (CPU)** | **88–99 ms/frame (~31%)** | `58–65 ms` buffer copies (118 MB/frame), `27–53 ms` heap pass (15.25 ms descriptor sync), `5–9 ms` textures |
| **Copy-back** | **32–34 ms/frame (~11%)** | `47.2 MB` fetched on `QUEUE2` (`3.5 ms` GPU / `5.0 ms` wait) + `29.4 MB` CPU shadow compare |
| **Asset / texture streaming** | **Slow** | `7,174` queued heap-indexed texture slots (`3,094` waiting for memory, `3,467` partial) |

### Bottleneck Diagnosis & Next Candidate PRs to Benchmark on RTX 3080 Ti
1. **GPU indirect clamp & barrier overhead (175 ms/frame):**
   - `fxceless/victory-fork#995` (*indirect clamps in runs, landing fetches from card slots, re-check fix — reported 2.5 -> 7.5 fps on RTX 5070 Ti*)
   - `fxceless/victory-fork#1016` (*specialize indirect argument clamp for single records*)
2. **Slow asset/texture loading & CPU heap/copy overhead (88–99 ms/frame):**
   - `fxceless/victory-fork#1007` (*async background worker for heap-indexed texture uploads, `VICTORY_HEAP_WORKER=1`*)
   - `fxceless/victory-fork#1018` (*skip CPU BLAKE3 texture hashing via xmem dirty-generation digests, `VICTORY_TEXTURE_DIRTY_GEN=1`*)
   - `fxceless/victory-fork#1005` (*notify GPU after unmapping view, not before*)
   - `fxceless/victory-fork#1020` (*park emulated MONITORX/MWAITX waits on GPU fence signals*)
3. **NVIDIA Wave32 shader correctness:**
   - `fxceless/victory-fork#1002` (*tiled cascade shadow programs translate exactly on 32-lane hosts*)
   - `fxceless/victory-fork#1006` (*code table starts after instructions — port of `Marvisak/VIctory#2143`*)
   - `fxceless/victory-fork#999` (*water texture readback stale-write guard*)
