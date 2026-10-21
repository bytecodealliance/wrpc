package test

import (
	"context"

	"driver/test/exports/test/flags_widths/to_test"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) RoundtripFlag17(ctx context.Context, a *to_test.Flag17, b uint8) (*to_test.Flag17, uint8, error) {
	return a, b, nil
}

func (handler) RoundtripFlag40(ctx context.Context, a *to_test.Flag40, b uint8) (*to_test.Flag40, uint8, error) {
	return a, b, nil
}
