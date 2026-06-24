//@ wasmtime-flags = '-Wcomponent-model-map'

package test

import (
	"context"

	wrpc "wrpc.io/go"

	"driver/test/exports/test/maps/to_test"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) NamedRoundtrip(ctx context.Context, a to_test.NamesById) (to_test.IdsByName, error) {
	result := make(to_test.IdsByName)
	for id, name := range a {
		result[name] = id
	}
	return result, nil
}

func (handler) BytesRoundtrip(ctx context.Context, a to_test.BytesByName) (to_test.BytesByName, error) {
	return a, nil
}

func (handler) EmptyRoundtrip(ctx context.Context, a to_test.NamesById) (to_test.NamesById, error) {
	return a, nil
}

func (handler) OptionRoundtrip(ctx context.Context, a map[string]*uint32) (map[string]*uint32, error) {
	return a, nil
}

func (handler) RecordRoundtrip(ctx context.Context, a *to_test.LabeledEntry) (*to_test.LabeledEntry, error) {
	return a, nil
}

func (handler) InlineRoundtrip(ctx context.Context, a map[uint32]string) (map[string]uint32, error) {
	result := make(map[string]uint32)
	for k, v := range a {
		result[v] = k
	}
	return result, nil
}

func (handler) LargeRoundtrip(ctx context.Context, a to_test.NamesById) (to_test.NamesById, error) {
	return a, nil
}

func (handler) MultiParamRoundtrip(ctx context.Context, a to_test.NamesById, b to_test.BytesByName) (to_test.IdsByName, to_test.BytesByName, error) {
	ids := make(to_test.IdsByName)
	for id, name := range a {
		ids[name] = id
	}
	return ids, b, nil
}

func (handler) NestedRoundtrip(ctx context.Context, a map[string]map[uint32]string) (map[string]map[uint32]string, error) {
	return a, nil
}

func (handler) VariantRoundtrip(ctx context.Context, a *to_test.MapOrString) (*to_test.MapOrString, error) {
	return a, nil
}

func (handler) ResultRoundtrip(ctx context.Context, a *wrpc.Result[to_test.NamesById, string]) (*wrpc.Result[to_test.NamesById, string], error) {
	return a, nil
}

func (handler) TupleRoundtrip(ctx context.Context, a *wrpc.Tuple2[to_test.NamesById, uint64]) (to_test.NamesById, uint64, error) {
	return a.V0, a.V1, nil
}

func (handler) SingleEntryRoundtrip(ctx context.Context, a to_test.NamesById) (to_test.NamesById, error) {
	return a, nil
}
