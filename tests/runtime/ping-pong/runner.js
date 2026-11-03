import assert from "node:assert/strict";

import { ping, pong } from "./my/test/i.js";

export async function run(connect) {
  const x = Promise.withResolvers();
  const [{ result, finished }] = await Promise.all([
    ping(await connect(), x.promise, "world"),
    x.resolve("hello"),
  ]);
  const [m2] = await Promise.all([result, finished]);
  assert.equal(m2, "helloworld");

  const y = Promise.withResolvers();
  const [res] = await Promise.all([pong(await connect(), y.promise), y.resolve(m2)]);
  assert.equal(res.result, "helloworld");
  await res.finished;
}
