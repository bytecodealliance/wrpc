import * as i from "./my/test/i.js";

export async function run(connect) {
  for (const f of [i.cancelBeforeRead, i.cancelAfterRead]) {
    const { promise, resolve } = Promise.withResolvers();
    const [{ finished }] = await Promise.all([f(await connect(), promise), resolve(0)]);
    await finished;
  }

  const data = Promise.withResolvers();
  const signal = Promise.withResolvers();
  const [{ finished }] = await Promise.all([
    i.startReadThenCancel(await connect(), data.promise, signal.promise),
    (async () => {
      signal.resolve();
      data.resolve(4);
    })(),
  ]);
  await finished;
}
