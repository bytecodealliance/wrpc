package test

import (
	"context"
	"fmt"
	"slices"
	"sync/atomic"

	wrpc "wrpc.io/go"

	to_test "driver/test/exports/test/flavorful/to_test"
)

func ptr[T any](v T) *T {
	return &v
}

type handler struct {
	first atomic.Bool
}

func NewHandler() *handler {
	h := &handler{}
	h.first.Store(true)
	return h
}

func (*handler) FListInRecord1(ctx context.Context, a *to_test.ListInRecord1) error {
	if a.A != "list_in_record1" {
		return fmt.Errorf("unexpected: %q", a.A)
	}
	return nil
}

func (*handler) FListInRecord2(ctx context.Context) (*to_test.ListInRecord2, error) {
	return &to_test.ListInRecord2{A: "list_in_record2"}, nil
}

func (*handler) FListInRecord3(ctx context.Context, a *to_test.ListInRecord3) (*to_test.ListInRecord3, error) {
	if a.A != "list_in_record3 input" {
		return nil, fmt.Errorf("unexpected: %q", a.A)
	}
	return &to_test.ListInRecord3{A: "list_in_record3 output"}, nil
}

func (*handler) FListInRecord4(ctx context.Context, a *to_test.ListInRecord4) (*to_test.ListInRecord4, error) {
	if a.A != "input4" {
		return nil, fmt.Errorf("unexpected: %q", a.A)
	}
	return &to_test.ListInRecord4{A: "result4"}, nil
}

func (*handler) FListInVariant1(ctx context.Context, a *string, b *to_test.ListInVariant1V2) error {
	if a == nil || *a != "foo" {
		return fmt.Errorf("unexpected a: %v", a)
	}
	if b.Err == nil || *b.Err != "bar" {
		return fmt.Errorf("unexpected b: %v", b)
	}
	return nil
}

func (*handler) FListInVariant2(ctx context.Context) (*string, error) {
	return ptr("list_in_variant2"), nil
}

func (*handler) FListInVariant3(ctx context.Context, a *string) (*string, error) {
	if a == nil || *a != "input3" {
		return nil, fmt.Errorf("unexpected: %v", a)
	}
	return ptr("output3"), nil
}

func (h *handler) ErrnoResult(ctx context.Context) (*wrpc.Result[struct{}, to_test.MyErrno], error) {
	if h.first.Swap(false) {
		return wrpc.Err[struct{}](to_test.MyErrno_B), nil
	}
	return wrpc.Ok[to_test.MyErrno](struct{}{}), nil
}

func (*handler) ListTypedefs(ctx context.Context, a string, c []string) ([]uint8, []string, error) {
	if a != "typedef1" {
		return nil, nil, fmt.Errorf("unexpected a: %q", a)
	}
	if !slices.Equal(c, []string{"typedef2"}) {
		return nil, nil, fmt.Errorf("unexpected c: %v", c)
	}
	return []uint8("typedef3"), []string{"typedef4"}, nil
}

func (*handler) ListOfVariants(ctx context.Context, bools []bool, results []*wrpc.Result[struct{}, struct{}], enums []to_test.MyErrno) ([]bool, []*wrpc.Result[struct{}, struct{}], []to_test.MyErrno, error) {
	if !slices.Equal(bools, []bool{true, false}) {
		return nil, nil, nil, fmt.Errorf("unexpected bools: %v", bools)
	}
	if len(results) != 2 || results[0].Ok == nil || results[1].Err == nil {
		return nil, nil, nil, fmt.Errorf("unexpected results: %v", results)
	}
	if !slices.Equal(enums, []to_test.MyErrno{to_test.MyErrno_Success, to_test.MyErrno_A}) {
		return nil, nil, nil, fmt.Errorf("unexpected enums: %v", enums)
	}
	return []bool{false, true},
		[]*wrpc.Result[struct{}, struct{}]{
			wrpc.Err[struct{}](struct{}{}),
			wrpc.Ok[struct{}](struct{}{}),
		},
		[]to_test.MyErrno{to_test.MyErrno_A, to_test.MyErrno_B},
		nil
}
