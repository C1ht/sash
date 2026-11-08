SASH DESIGN SPEC (frozen 2026-11-07, from the design canvas)

Scale: Fibonacci 1 2 3 5 8 13 21 34 55 89 144 233 377 610 987 1597.
Radius = min(shorter side x 0.236, 21). Exceptions set by hand: dock and
launcher panel 34; resizable windows 13. Inner radius = outer radius
minus padding where nested (dock 34, padding 21, icons 13).

Colours, dark: chrome/bar #15191C, unfocused bar #1B2024, page #1D2226,
raised #22282C, card #23292E, hairline #2B3238, text white 92%, secondary
white 62%, accent #5EC4BE, accent tile #163331, text on accent fill
#14181B, close hover #FF453A, overlays white 5/8/12%, window outline white
8% (focused 14%), inactive dim black 14%.
Colours, light: chrome #E5E9EC, unfocused bar #EEF1F3, page #FFFFFF,
raised #F3F5F7, hairline #DCE0E5, text #1B2126, secondary #5A636A, accent
#0B6E6E, accent tile #DCEFEE, text on accent fill #FFFFFF, overlays #1B2126
at 5/8/12%, window outline black 10% (focused 18%), inactive dim black 4%.
Default accent is teal (matches the browser); blue #0A84FF/#007AFF stays
a preset.

Type: Manrope. Ramp 13 (secondary, labels, titles), 17 (body, rows),
21 (search, dialog titles), 34 (display). Weights 400/500/600. Section
headers 13/600. Clock 17/500 tabular, date 13/400. Terminal 13 on 17.

Gaps: outer 13, inner 8 (each interior edge insets 4), everywhere:
tiling, snap zones, maximize. Fullscreen none. Dock margin 13, dock zone
13 + 97 = 110 from the screen edge; windows start 123 from the screen
edge (zone 110 + outer gap 13), so 13 px from the dock. Every tile is a
fully rounded rectangle with a hairline on all edges (the old corner rule
and the no-border-on-screen-edge rule are removed). Minimum tile 233 x 144.

Windows: bar 34 (buttons 34, order close, maximize, minimize; glyphs 15 at
2 px; hover square 21, radius 5; title centred, 13/500 focused, 13/400
not; no hairline under the bar). Focus is shown by bar tone (#15191C
focused, #1B2024 not), outline, shadow and inactive dim, never a coloured
outline. Tiled windows have no bar.
Shadows (only on things that float): window unfocused 0 1 2 black 20% +
0 8 21 black 30%; focused 0 1 2 25% + 0 21 55 45%; glass panels 0 1 2 20%
+ 0 21 55 25%. Light theme: 10/14, 12/22, 10/14.

Interaction constants (logical px): resize zone 8 inside the outline,
corner zone 21 x 21; snap edge trigger 8, snap corner trigger 55; edge
snap distance 13; visibility rule 55 (a moved window keeps at least 55 px
on the output per axis); keyboard move/resize step 34; cascade 34; minimum
tile 233 x 144. Cursor size: a setting (input group, per user): medium 24
(default), large 36, extra large 48, from the sizes the Adwaita cursor
theme ships (24, 30, 36, 48, 72, 96; nothing below 24); bitmaps are never
scaled; at output scales 1.25 to 2 the next shipped size up is used, so the
apparent size holds.

Glass B (chosen): dark tint #15191C at 88%, blur 80, saturation 100%,
edge white 10%, top highlight white 16%, fine monochrome grain, soft
(tune by eye, about 4%). Light tint #F3F5F7 at 90%, same blur (NOT YET
DRAWN, verify). Text on glass 92% (85% minimum); placeholders 62%; no 55%
text on glass. Reduce transparency: opaque chrome colour. A tint below
about 68% fails 4.5:1 over a white wallpaper (my calculation, to be
confirmed by a test). Glass A (82%, blur 55, saturation 110%) was rejected.

Dock: 97 high (55 icon + 2 x 21), radius 34, gap 8, one separator, then
minimized windows (dimmed icon plus a 21 px window badge), tray glyphs
24 px at 1.75 stroke, clock. Indicators: none (not running), 5 px dot
(running), 21 x 5 accent pill (focused; also launcher open). Position is
a setting: bottom, left or right, per monitor. Side docks stack
vertically, indicators on the outer edge, launcher opens beside the dock,
vertically centred.

Launcher: 987 x 610, radius 34, padding 34, glass B. Search 55 high,
radius 13, text 21. Content area 377: pinned grid of 5 columns (cells 124 x
152, icons 89 radius 21, labels 13), headers 13/600; Recent plate 233 x 377
radius 21 padding 21, rows 55 (icon 34 radius 8, name 17, time 13); footer
pills 34 high radius 8, 55 above them. Selection: 8% cell fill. Keyboard
focus: 2 px accent ring 3 px off the icon. Bottom dock: centred, 21 above
the dock. No shadows on icons.

Icons (STAND-INS, a designer must replace them): ink plate gradient
#1A1D21 to #0D0D0F, inner edge white 10%, top highlight 14%, glyph stroke
8 at 89 px, two tones white 96%/45%, no accent colour. The browser icon
is the logo (white ring on #0D0D0F). Chrome glyphs: Tabler (MIT) at 1.75 px.

Wallpaper: native 5120 px wide or more, cool graphite/teal family; a
separate light default. Cover-fit, never stretched.

Open: dock-on-side details beyond the mirror; the browser draws its own
tab strip under our 34 px bar (keep both for now); cursor size default 24,
now a setting; light glass B; real icon set.
