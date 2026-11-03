import * as i from "./my/test/i.js";

export async function run(connect) {
  for (const f of [i.readFuture, i.dropFuture]) {
    const { promise, resolve } = Promise.withResolvers();
    const [{ finished }] = await Promise.all([f(await connect(), promise), resolve()]);
    await finished;
  }
}
