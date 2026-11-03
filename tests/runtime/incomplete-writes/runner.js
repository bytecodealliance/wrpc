import assert from "node:assert/strict";

import { LeafThing } from "./my/test/leaf-interface.js";
import * as test from "./my/test/test-interface.js";

export async function run(connect) {
  for (const [Thing, shortReads] of [
    [test.TestThing, test.shortReadsTest],
    [LeafThing, test.shortReadsLeaf],
  ]) {
    const things = [
      await Thing.new(await connect(), "a"),
      await Thing.new(await connect(), "b"),
      await Thing.new(await connect(), "c"),
    ];
    const { result, finished } = await shortReads(await connect(), [things]);
    const received = [];
    for await (const chunk of result) received.push(...chunk);
    await finished;
    assert.equal(received.length, 3);
    assert.equal(await Thing.get(await connect(), received[0]), "a");
    assert.equal(await Thing.get(await connect(), received[1]), "b");
    assert.equal(await Thing.get(await connect(), received[2]), "c");
  }

  for (const [Thing, droppedReader] of [
    [test.TestThing, test.droppedReaderTest],
    [LeafThing, test.droppedReaderLeaf],
  ]) {
    const thing = await Thing.new(await connect(), "a");
    const {
      result: [, f2],
      finished,
    } = await droppedReader(await connect(), Promise.resolve(thing), Promise.resolve(thing));
    const received = await f2;
    await finished;
    assert.equal(await Thing.get(await connect(), received), "a");
  }
}
