package runner

import (
	"context"
	"fmt"
	"slices"

	wrpc "wrpc.io/go"

	"driver/runner/test/flavorful/to_test"
)

func ptr[T any](v T) *T {
	return &v
}

func Run(ctx context.Context, c wrpc.Invoker) error {
	must0(to_test.FListInRecord1(ctx, c, &to_test.ListInRecord1{A: "list_in_record1"}))

	assertEqual(must(to_test.FListInRecord2(ctx, c)).A, "list_in_record2")

	assertEqual(must(to_test.FListInRecord3(ctx, c, &to_test.ListInRecord3{A: "list_in_record3 input"})).A, "list_in_record3 output")

	assertEqual(must(to_test.FListInRecord4(ctx, c, &to_test.ListInAlias{A: "input4"})).A, "result4")

	must0(to_test.FListInVariant1(ctx, c, ptr("foo"), wrpc.Err[struct{}]("bar")))

	assertEqual(*must(to_test.FListInVariant2(ctx, c)), "list_in_variant2")

	assertEqual(*must(to_test.FListInVariant3(ctx, c, ptr("input3"))), "output3")

	res1 := must(to_test.ErrnoResult(ctx, c))
	assert(res1.Err != nil)
	res2 := must(to_test.ErrnoResult(ctx, c))
	assert(res2.Ok != nil)

	{
		a, b, err := to_test.ListTypedefs(ctx, c, "typedef1", []string{"typedef2"})
		if err != nil {
			panic(err)
		}
		assert(slices.Equal(a, []byte("typedef3")))
		assertEqual(len(b), 1)
		assertEqual(b[0], "typedef4")
	}

	{
		a, b, cc, err := to_test.ListOfVariants(ctx, c,
			[]bool{true, false},
			[]*wrpc.Result[struct{}, struct{}]{
				wrpc.Ok[struct{}](struct{}{}),
				wrpc.Err[struct{}](struct{}{}),
			},
			[]to_test.MyErrno{to_test.MyErrno_Success, to_test.MyErrno_A},
		)
		if err != nil {
			panic(err)
		}
		assert(slices.Equal(a, []bool{false, true}))
		assertEqual(len(b), 2)
		assert(b[0].Err != nil)
		assert(b[1].Ok != nil)
		assert(slices.Equal(cc, []to_test.MyErrno{to_test.MyErrno_A, to_test.MyErrno_B}))
	}
	return nil
}

func must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}

func must0(err error) {
	if err != nil {
		panic(err)
	}
}

func assertEqual[T comparable](a T, b T) {
	if a != b {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}

func assert(v bool) {
	if !v {
		panic("assertion failed")
	}
}
