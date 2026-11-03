package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/exports_"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	res := must(exports_.TestImports(ctx, c))
	if res.Err != nil {
		panic(*res.Err)
	}

	x := must(exports_.NewX(ctx, c, 5))
	assertEqual(must(exports_.X_GetA(ctx, c, x.Borrow())), 5)
	if err := exports_.X_SetA(ctx, c, x.Borrow(), 10); err != nil {
		return err
	}
	assertEqual(must(exports_.X_GetA(ctx, c, x.Borrow())), 10)
	z1 := must(exports_.NewZ(ctx, c, 10))
	assertEqual(must(exports_.Z_GetA(ctx, c, z1.Borrow())), 10)
	z2 := must(exports_.NewZ(ctx, c, 20))
	assertEqual(must(exports_.Z_GetA(ctx, c, z2.Borrow())), 20)

	xadd := must(exports_.X_Add(ctx, c, x, 5))
	assertEqual(must(exports_.X_GetA(ctx, c, xadd.Borrow())), 15)

	zadd := must(exports_.Add(ctx, c, z1.Borrow(), z2.Borrow()))
	assertEqual(must(exports_.Z_GetA(ctx, c, zadd.Borrow())), 30)

	if _, err := exports_.Z_NumDropped(ctx, c); err != nil {
		return err
	}
	return exports_.Consume(ctx, c, xadd)
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
