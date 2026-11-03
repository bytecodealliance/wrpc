import * as i from "./a/b/i.js";

export async function run(connect) {
  await i.f(await connect());
}
