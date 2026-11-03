import assert from "node:assert/strict";

import * as toTest from "./test/maps/to-test.js";

export async function run(connect) {
  const idsByName = await toTest.namedRoundtrip(
    await connect(),
    new Map([
      [1, "one"],
      [1, "uno"],
      [2, "two"],
    ]),
  );
  assert.equal(idsByName.get("uno"), 1);
  assert.equal(idsByName.get("two"), 2);
  assert.equal(idsByName.get("one"), undefined);

  const bytesByName = await toTest.bytesRoundtrip(
    await connect(),
    new Map([
      ["hello", new TextEncoder().encode("world")],
      ["bin", Uint8Array.of(0, 1, 2)],
    ]),
  );
  assert.deepEqual(bytesByName.get("hello"), new TextEncoder().encode("world"));
  assert.deepEqual(bytesByName.get("bin"), Uint8Array.of(0, 1, 2));

  assert.equal((await toTest.emptyRoundtrip(await connect(), new Map())).size, 0);

  const options = new Map([
    ["some", 42],
    ["none", undefined],
  ]);
  assert.deepEqual(await toTest.optionRoundtrip(await connect(), options), options);

  const values = new Map([
    [10, "ten"],
    [20, "twenty"],
  ]);
  const entry = await toTest.recordRoundtrip(await connect(), { label: "test-label", values });
  assert.equal(entry.label, "test-label");
  assert.deepEqual(entry.values, values);

  assert.deepEqual(
    await toTest.inlineRoundtrip(
      await connect(),
      new Map([
        [1, "one"],
        [2, "two"],
      ]),
    ),
    new Map([
      ["one", 1],
      ["two", 2],
    ]),
  );

  const large = new Map(Array.from({ length: 100 }, (_, i) => [i, `value-${i}`]));
  assert.deepEqual(await toTest.largeRoundtrip(await connect(), large), large);

  const [ids, bytesOut] = await toTest.multiParamRoundtrip(
    await connect(),
    new Map([
      [1, "one"],
      [2, "two"],
    ]),
    new Map([["key", Uint8Array.of(42)]]),
  );
  assert.deepEqual(
    ids,
    new Map([
      ["one", 1],
      ["two", 2],
    ]),
  );
  assert.deepEqual(bytesOut, new Map([["key", Uint8Array.of(42)]]));

  const outer = new Map([
    [
      "group-a",
      new Map([
        [1, "one"],
        [2, "two"],
      ]),
    ],
    ["group-b", new Map([[10, "ten"]])],
  ]);
  assert.deepEqual(await toTest.nestedRoundtrip(await connect(), outer), outer);

  const asMap = await toTest.variantRoundtrip(await connect(), {
    kind: "as-map",
    value: new Map([[1, "one"]]),
  });
  assert.equal(asMap.kind, "as-map");
  assert.equal(asMap.value.get(1), "one");
  const asString = await toTest.variantRoundtrip(await connect(), {
    kind: "as-string",
    value: "hello",
  });
  assert.equal(asString.kind, "as-string");
  assert.equal(asString.value, "hello");

  const ok = await toTest.resultRoundtrip(await connect(), {
    kind: "ok",
    value: new Map([[5, "five"]]),
  });
  assert.equal(ok.kind, "ok");
  assert.equal(ok.value.get(5), "five");
  const err = await toTest.resultRoundtrip(await connect(), { kind: "error", value: "bad input" });
  assert.equal(err.kind, "error");
  assert.equal(err.value, "bad input");

  const [resultMap, resultNum] = await toTest.tupleRoundtrip(await connect(), [
    new Map([[7, "seven"]]),
    42n,
  ]);
  assert.deepEqual(resultMap, new Map([[7, "seven"]]));
  assert.equal(resultNum, 42n);

  const single = new Map([[99, "ninety-nine"]]);
  assert.deepEqual(await toTest.singleEntryRoundtrip(await connect(), single), single);
}
