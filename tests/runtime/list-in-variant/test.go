package test

import (
	"context"
	"strings"

	wrpc "wrpc.io/go"

	"driver/test/exports/test/list_in_variant/to_test"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) ListInOption(ctx context.Context, data []string) (string, error) {
	if data == nil {
		return "none", nil
	}
	return strings.Join(data, ","), nil
}

func (handler) ListInVariant(ctx context.Context, data *to_test.PayloadOrEmpty) (string, error) {
	if list, ok := data.GetWithData(); ok {
		return strings.Join(list, ","), nil
	}
	return "empty", nil
}

func (handler) ListInResult(ctx context.Context, data *wrpc.Result[[]string, string]) (string, error) {
	if data.Err != nil {
		return "err:" + *data.Err, nil
	}
	return strings.Join(*data.Ok, ","), nil
}

func (handler) ListInOptionWithReturn(ctx context.Context, data []string) (*to_test.Summary, error) {
	if data == nil {
		return &to_test.Summary{Count: 0, Label: "none"}, nil
	}
	return &to_test.Summary{Count: uint32(len(data)), Label: strings.Join(data, ",")}, nil
}

func (handler) TopLevelList(ctx context.Context, items []string) (string, error) {
	return strings.Join(items, ","), nil
}
