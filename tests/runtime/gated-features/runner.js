//@ args = '--features y'

import * as bindings from "./foo/bar/bindings.js";

export async function run(connect) {
  await bindings.y(await connect());
  await bindings.z(await connect());
}
