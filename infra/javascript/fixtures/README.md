# Map fixtures

`place.pbf.hex` and `extreme-polygon.pbf.hex` are synthetic codec regression inputs.

The tiles reproduce rejected overview backgrounds. They are unmodified OpenFreeMap tiles from snapshot
`20260913_164504_pt`, using the OpenMapTiles schema and © OpenStreetMap contributors' data
([ODbL attribution](https://www.openstreetmap.org/copyright)).

| Fixture                | Source                                                                    | SHA-256                                                            |
| ---------------------- | ------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| `world-z0.pbf`         | <https://tiles.openfreemap.org/planet/20260913_164504_pt/0/0/0.pbf>       | `055ff033cd389b87d7767ca74e9a7f334d279a0d8e7951107b4e2752c86f6ae9` |
| `europe-africa-z2.pbf` | <https://tiles.openfreemap.org/planet/20260913_164504_pt/2/1/1.pbf>       | `c3912641de47845541b142d193f834627098bb2da5908c40f4117f61c4bdb38f` |
| `europe-africa-z3.pbf` | <https://tiles.openfreemap.org/planet/20260913_164504_pt/3/3/2.pbf>       | `6b49b0b077807c1137d606061659a93eb6df429ebff947235e121c136e1dbe6c` |
| `asia-z3.pbf`          | <https://tiles.openfreemap.org/planet/20260913_164504_pt/3/5/2.pbf>       | `3fe854de34c2acf1376ef552701859cf68ea667010bae4d507a3cc73a28b4684` |
| `london-z7.pbf`        | <https://tiles.openfreemap.org/planet/20260913_164504_pt/7/62/44.pbf>     | `9ab6529cd384d770111193556beae05f48f4400fc316e92ace7087271daf373b` |
| `london-z8.pbf`        | <https://tiles.openfreemap.org/planet/20260913_164504_pt/8/127/85.pbf>    | `9179c9aad4a0e9c577926e2acc925226a33a9f4b79596f88980e92269950d87a` |
| `london-z9.pbf`        | <https://tiles.openfreemap.org/planet/20260913_164504_pt/9/257/169.pbf>   | `f318a1ce15e329ba4e61be07c6d2b4aa6e4cbb5b1841c580c144e50b886faf08` |
| `london-z11.pbf`       | <https://tiles.openfreemap.org/planet/20260913_164504_pt/11/1023/680.pbf> | `eb2b2baf8df0a920f5cab5c30563a88550fb8563fec880fbaade7eb7321de6bb` |
| `london-z10.pbf`       | <https://tiles.openfreemap.org/planet/20260913_164504_pt/10/511/340.pbf>  | `950b78e146298e600406a48a2d4b2a43129049f5de4bd6fae0119d1dd2037ce1` |

With the dark basemap, z11 produces 16,393 shapes from 44,418 input points. Z10 produces 28,911 shapes from 80,960 input
points and 10,174,104 GPU mesh bytes before labels. These exceed the former count and 8 MiB upload limits. Decode
geometry, styled storage, and final uploads now have separate budgets; style expansion is not input geometry. Native
tests preserve shape/label parity, and emitted-WASM tests exercise the tiles in both themes without network access.

Z7 and z9 contain water polygons with outer rings of 4,684 and 5,527 points. Z8 contains a land-use polygon whose outer
ring and holes together exceed 4,096 points. These reproduce the mistaken use of the line budget for polygons; polygons
now have a separate 8,192-point bound including holes. The aggregate tile point limit remains unchanged.

Low-zoom tiles carry large multilingual dictionaries. Europe/Africa z2 needs 4,008,208 bytes of codec element storage on
the native test target; Europe/Africa z3 needs 2,865,608, and Asia z3 needs 3,993,160. They exceeded the old 2 MiB
metadata allowance. Z2 and Asia z3 also contain 66,500 and 68,387 property references, exceeding the old 65,536 limit.
The new limits are 8 MiB of codec element storage and 131,072 property references; encoded input, expanded property
bytes, geometry, and upload limits are unchanged. Small synthetic tiles still test metadata amplification rejection.
