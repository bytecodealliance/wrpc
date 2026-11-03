import * as i from "./my/test/i.js";

export async function run(connect) {
  const { finished } = await i.readStream(
    await connect(),
    (async function* () {
      yield Uint8Array.of(10, 20, 30);
    })(),
  );
  await finished;
}
