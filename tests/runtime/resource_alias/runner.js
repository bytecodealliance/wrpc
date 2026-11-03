import * as e1 from "./test/resource-alias/e1.js";
import * as e2 from "./test/resource-alias/e2.js";

export async function run(connect) {
  const x = await e1.X.new(await connect(), 42);
  await e1.a(await connect(), { x });

  const fooE2 = { x: await e1.X.new(await connect(), 7) };
  const barE2 = { x: await e1.X.new(await connect(), 8) };
  const y = await e1.X.new(await connect(), 8);
  await e2.a(await connect(), fooE2, barE2, y);
}
