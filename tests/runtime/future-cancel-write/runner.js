import { readAndDrop, takeThenDrop } from "./my/test/i.js";

export async function run(connect) {
  const { promise, resolve } = Promise.withResolvers();
  const [taken] = await Promise.all([takeThenDrop(await connect(), promise), resolve("hello")]);
  await taken.finished;

  const { finished } = await readAndDrop(await connect(), Promise.resolve("hello3"));
  await finished;
}
