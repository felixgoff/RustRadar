/**
 * Trees for the realistic map as Streets GL draws them (its tree.glb): two
 * upright cards crossed at right angles, a unit wide and a unit tall,
 * standing on the ground, each showing a picture of a whole tree (Streets
 * GL's tree textures, which realistic.ts shades as a rounded crown); and,
 * for a crown seen from above (where the upright cards are edge-on), a level
 * one across it, showing the picture's crown. The shader turns each card's
 * normal to face the camera, so one quad a card does for both its sides.
 */
import { Geometry } from "@luma.gl/engine";

export type CardArrays = { positions: number[]; normals: number[]; texCoords: number[] };

/** The crown's card: this high up the tree, this wide (of the tree's width), and where the crown is in the picture (u, v). */
const CROWN = { z: 0.6, width: 0.72, u: [0.14, 0.86], v: [0.1, 0.62] };

/** The cards: one across east–west (facing south), one across north–south (facing east), and the crown's, level. */
export function cardArrays(): CardArrays {
  const a: CardArrays = { positions: [], normals: [], texCoords: [] };
  const cards: [number[], number[]][] = [
    [
      [1, 0, 0],
      [0, -1, 0],
    ],
    [
      [0, 1, 0],
      [1, 0, 0],
    ],
  ];
  for (const [across, facing] of cards) {
    // (the picture's top row first: v is 0 at the top of the tree)
    for (const [u, z] of [[0, 0], [1, 0], [1, 1], [0, 0], [1, 1], [0, 1]]) {
      a.positions.push(across[0] * (u - 0.5), across[1] * (u - 0.5), z);
      a.normals.push(...facing);
      a.texCoords.push(u, 1 - z);
    }
  }
  const half = CROWN.width / 2;
  for (const [u, v] of [[0, 0], [1, 0], [1, 1], [0, 0], [1, 1], [0, 1]]) {
    a.positions.push((u - 0.5) * 2 * half, (v - 0.5) * 2 * half, CROWN.z);
    a.normals.push(0, 0, 1);
    a.texCoords.push(CROWN.u[0] + (CROWN.u[1] - CROWN.u[0]) * u, CROWN.v[1] - (CROWN.v[1] - CROWN.v[0]) * v);
  }
  return a;
}

let card: Geometry | null = null;
/** The trees' mesh, built once. */
export function treeCard(): Geometry {
  if (card) return card;
  const a = cardArrays();
  return (card = new Geometry({
    topology: "triangle-list",
    attributes: {
      positions: { size: 3, value: new Float32Array(a.positions) },
      normals: { size: 3, value: new Float32Array(a.normals) },
      texCoords: { size: 2, value: new Float32Array(a.texCoords) },
      // the instance's colour as it is (deck.gl multiplies the two)
      colors: { size: 3, value: new Float32Array(a.positions.length).fill(1) },
    },
  }));
}
