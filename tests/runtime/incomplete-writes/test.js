async function* shortReads(s) {
  const things = [];
  for await (const chunk of s) things.push(...chunk);
  yield things;
}

const droppedReader = (f1, f2) => [f2, f2];

export const handlers = () => [
  {
    shortReadsTest: shortReads,
    shortReadsLeaf: shortReads,
    droppedReaderTest: droppedReader,
    droppedReaderLeaf: droppedReader,
    TestThing: {
      new: (s) => new TextEncoder().encode(s),
      get: (self) => new TextDecoder().decode(self),
    },
  },
];
