import * as i from "./my/test/i.js";

export async function run(connect) {
  const { finished } = await i.readStream(
    await connect(),
    (async function* () {
      yield Uint8Array.of(0);
      yield Uint8Array.of(1, 2);
      yield Uint8Array.of(3, 4);
    })(),
  );
  await finished;
}
