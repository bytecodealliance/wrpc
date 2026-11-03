//@ wasmtime-flags = '-Wcomponent-model-async'

package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	leaf "driver/runner/my/test/leaf_interface"
	test "driver/runner/my/test/test_interface"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	{
		tx := make(chan []wrpc.Own[test.TestThing], 1)
		stream, errs, err := test.ShortReadsTest(ctx, c, tx)
		if err != nil {
			return err
		}

		// Write the things all at once.  The callee will read them only
		// one at a time.
		tx <- []wrpc.Own[test.TestThing]{
			must(test.NewTestThing(ctx, c, "a")),
			must(test.NewTestThing(ctx, c, "b")),
			must(test.NewTestThing(ctx, c, "c")),
		}
		close(tx)

		var things []wrpc.Own[test.TestThing]
		for chunk := range stream {
			things = append(things, chunk...)
		}
		for err := range errs {
			return err
		}

		assertEqual(len(things), 3)
		assertEqual(must(test.TestThing_Get(ctx, c, things[0].Borrow())), "a")
		assertEqual(must(test.TestThing_Get(ctx, c, things[1].Borrow())), "b")
		assertEqual(must(test.TestThing_Get(ctx, c, things[2].Borrow())), "c")
	}

	{
		tx := make(chan []wrpc.Own[leaf.LeafThing], 1)
		stream, errs, err := test.ShortReadsLeaf(ctx, c, tx)
		if err != nil {
			return err
		}

		// Write the things all at once.  The callee will read them only
		// one at a time.
		tx <- []wrpc.Own[leaf.LeafThing]{
			must(leaf.NewLeafThing(ctx, c, "a")),
			must(leaf.NewLeafThing(ctx, c, "b")),
			must(leaf.NewLeafThing(ctx, c, "c")),
		}
		close(tx)

		var things []wrpc.Own[leaf.LeafThing]
		for chunk := range stream {
			things = append(things, chunk...)
		}
		for err := range errs {
			return err
		}

		assertEqual(len(things), 3)
		assertEqual(must(leaf.LeafThing_Get(ctx, c, things[0].Borrow())), "a")
		assertEqual(must(leaf.LeafThing_Get(ctx, c, things[1].Borrow())), "b")
		assertEqual(must(leaf.LeafThing_Get(ctx, c, things[2].Borrow())), "c")
	}

	{
		tx1 := make(chan wrpc.Own[test.TestThing], 1)
		tx2 := make(chan wrpc.Own[test.TestThing], 1)
		_, f2, errs, err := test.DroppedReaderTest(ctx, c, tx1, tx2)
		if err != nil {
			return err
		}

		// Write a thing to the first future, the read end of which the
		// callee will drop without reading from, then write it again to
		// the second future, which the callee will read.
		thing := must(test.NewTestThing(ctx, c, "a"))
		tx1 <- thing
		close(tx1)
		tx2 <- thing
		close(tx2)

		// Drop the first future without reading from it and read from
		// the second future and assert it matches what we wrote above.
		thing = <-f2
		for err := range errs {
			return err
		}
		assertEqual(must(test.TestThing_Get(ctx, c, thing.Borrow())), "a")
	}

	{
		tx1 := make(chan wrpc.Own[leaf.LeafThing], 1)
		tx2 := make(chan wrpc.Own[leaf.LeafThing], 1)
		_, f2, errs, err := test.DroppedReaderLeaf(ctx, c, tx1, tx2)
		if err != nil {
			return err
		}

		// Write a thing to the first future, the read end of which the
		// callee will drop without reading from, then write it again to
		// the second future, which the callee will read.
		thing := must(leaf.NewLeafThing(ctx, c, "a"))
		tx1 <- thing
		close(tx1)
		tx2 <- thing
		close(tx2)

		// Drop the first future without reading from it and read from
		// the second future and assert it matches what we wrote above.
		thing = <-f2
		for err := range errs {
			return err
		}
		assertEqual(must(leaf.LeafThing_Get(ctx, c, thing.Borrow())), "a")
	}
	return nil
}

func must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}

func assertEqual[T comparable](a T, b T) {
	if a != b {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}
