import * as i from "./my/test/i.js";

export async function run(connect) {
  await i.readStream(
    await connect(),
    (async function* () {
      yield Uint8Array.of(10, 20, 30);
    })(),
  );
}
