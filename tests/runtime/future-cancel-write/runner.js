import { readAndDrop, takeThenDrop } from "./my/test/i.js";

export async function run(connect) {
  const { promise, resolve } = Promise.withResolvers();
  await Promise.all([takeThenDrop(await connect(), promise), resolve("hello")]);
  await readAndDrop(await connect(), Promise.resolve("hello3"));
}
