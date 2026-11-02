import * as toTest from "./test/many-arguments/to-test.js";

export async function run(connect) {
  const args = Array.from({ length: 16 }, (_, i) => BigInt(i + 1));
  await toTest.manyArguments(await connect(), ...args);
}
