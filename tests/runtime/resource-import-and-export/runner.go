package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/test/resource_import_and_export/test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	thing1 := must(test.NewThing(ctx, c, 42))

	// 42 + 1 (constructor) + 1 (constructor) + 2 (foo) + 2 (foo)
	assertEqual(must(test.Thing_Foo(ctx, c, thing1.Borrow())), 48)

	// 33 + 3 (bar) + 3 (bar) + 2 (foo) + 2 (foo)
	if err := test.Thing_Bar(ctx, c, thing1.Borrow(), 33); err != nil {
		return err
	}
	assertEqual(must(test.Thing_Foo(ctx, c, thing1.Borrow())), 43)

	thing2 := must(test.NewThing(ctx, c, 81))
	thing3 := must(test.Thing_Baz(ctx, c, thing1, thing2))
	assertEqual(
		must(test.Thing_Foo(ctx, c, thing3.Borrow())),
		33+3+3+81+1+1+2+2+4+1+2+4+1+1+2+2,
	)
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
