import assert from "node:assert/strict";

import { ping, pong } from "./my/test/i.js";

export async function run(connect) {
  const x = Promise.withResolvers();
  const [[result, io]] = await Promise.all([
    ping(await connect(), x.promise, "world"),
    x.resolve("hello"),
  ]);
  const [m2] = await Promise.all([result, io]);
  assert.equal(m2, "helloworld");

  const y = Promise.withResolvers();
  const [[res, io2]] = await Promise.all([pong(await connect(), y.promise), y.resolve(m2)]);
  assert.equal(res, "helloworld");
  await io2;
}
