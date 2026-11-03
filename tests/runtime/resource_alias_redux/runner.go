package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	a1 "driver/runner/test/resource_alias_redux/resource_alias1"
	a2 "driver/runner/test/resource_alias_redux/resource_alias2"
	"driver/runner/the_test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	thing1 := must(a1.NewThing(ctx, c, "Ni Hao"))
	result := must(the_test.Test(ctx, c, []wrpc.Own[a1.Thing]{thing1}))
	assertEqual(len(result), 1)
	assertEqual(must(a1.Thing_Get(ctx, c, result[0].Borrow())), "Ni Hao GuestThing GuestThing.get")

	thing2 := must(a1.NewThing(ctx, c, "Ciao"))
	result = must(a1.A(ctx, c, &a1.Foo{Thing: thing2}))
	assertEqual(len(result), 1)
	assertEqual(must(a1.Thing_Get(ctx, c, result[0].Borrow())), "Ciao GuestThing GuestThing.get")

	thing3 := must(a1.NewThing(ctx, c, "Ciao"))
	thing4 := must(a1.NewThing(ctx, c, "Aloha"))

	result = must(a2.B(ctx, c, &a2.Foo{Thing: thing3}, &a1.Foo{Thing: thing4}))
	assertEqual(len(result), 2)
	assertEqual(must(a1.Thing_Get(ctx, c, result[0].Borrow())), "Ciao GuestThing GuestThing.get")
	assertEqual(must(a1.Thing_Get(ctx, c, result[1].Borrow())), "Aloha GuestThing GuestThing.get")
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
