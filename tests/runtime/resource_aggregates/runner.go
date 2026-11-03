package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/test/resource_aggregates/to_test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	thing := func(v uint32) wrpc.Own[to_test.Thing] {
		return must(to_test.NewThing(ctx, c, v))
	}
	thing1 := thing(1)
	thing2 := thing(2)
	thing6 := thing(6)
	thing8 := thing(8)
	thing11 := thing(11)
	thing12 := thing(12)
	thing14 := thing(14)
	thing16 := thing(16)
	o1 := thing(13)
	o2 := thing14.Borrow()

	result := must(to_test.Foo(
		ctx,
		c,
		&to_test.R1{Thing: thing(0)},
		&to_test.R2{Thing: thing1.Borrow()},
		&to_test.R3{Thing1: thing2.Borrow(), Thing2: thing(3)},
		&wrpc.Tuple2[wrpc.Own[to_test.Thing], *to_test.R1]{V0: thing(4), V1: &to_test.R1{Thing: thing(5)}},
		thing6.Borrow(),
		to_test.NewV1Thing(thing(7)),
		to_test.NewV2Thing(thing8.Borrow()),
		[]wrpc.Own[to_test.Thing]{thing(9), thing(10)},
		[]wrpc.Borrow[to_test.Thing]{thing11.Borrow(), thing12.Borrow()},
		&o1,
		&o2,
		wrpc.Ok[struct{}](thing(15)),
		wrpc.Ok[struct{}](thing16.Borrow()),
	))
	var expected uint32
	for i := range uint32(17) {
		expected += i + 1
	}
	assertEqual(result, expected+3)
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
