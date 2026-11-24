import * as i from "./my/test/i.js";

export async function run(connect) {
  await i.readStream(
    await connect(),
    (async function* () {
      yield 1;
      yield 2;
    })(),
  );
}
