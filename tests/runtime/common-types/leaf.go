package leaf

import (
	"context"

	"driver/leaf/test/common/test_types"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) Wrap(ctx context.Context, flag *test_types.F1) (*test_types.R1, error) {
	if *flag == (test_types.F1{A: true}) {
		return &test_types.R1{A: 1, B: flag}, nil
	}
	return &test_types.R1{A: 2, B: flag}, nil
}

func (handler) VarF(ctx context.Context) (*test_types.V1, error) {
	return test_types.NewV1B(42), nil
}
