export const handlers = () => [
  {
    listInOption: (data) => (data === undefined ? "none" : data.join(",")),
    listInVariant: (data) => (data.kind === "with-data" ? data.value.join(",") : "empty"),
    listInResult: (data) => (data.kind === "ok" ? data.value.join(",") : `err:${data.value}`),
    listInOptionWithReturn: (data) =>
      data === undefined
        ? { count: 0, label: "none" }
        : { count: data.length, label: data.join(",") },
    topLevelList: (items) => items.join(","),
  },
];
