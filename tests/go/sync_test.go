//go:generate $WIT_BINDGEN_WRPC go --gofmt=false --world sync-client --out-dir bindings/sync_client --package wrpc.io/tests/go/bindings/sync_client ../wit

package integration_test

import (
	"context"
	"encoding/binary"
	"math"
	"strings"
	"testing"
	"time"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
	wrpc "wrpc.io/go"
	integration "wrpc.io/tests/go"
	"wrpc.io/tests/go/bindings/sync_client/foo"
	"wrpc.io/tests/go/bindings/sync_client/wrpc_test/integration/sync"
	"wrpc.io/tests/go/bindings/sync_server"
	"wrpc.io/tests/go/internal"
)

func serve(t *testing.T, f func(wrpc.Server) (func() error, error)) (context.Context, wrpc.Invoker) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	t.Cleanup(cancel)
	srv, client := internal.RunTCP(t, ctx)
	stop, err := f(srv)
	require.NoError(t, err, "failed to serve world")
	t.Cleanup(func() { assert.NoError(t, stop(), "failed to stop serving world") })
	internal.Accept(ctx, srv)
	return ctx, client
}

func TestSync(t *testing.T) {
	var h integration.SyncHandler
	ctx, client := serve(t, func(s wrpc.Server) (func() error, error) {
		return sync_server.Serve(s, h, h)
	})

	n, err := foo.F(ctx, client, "f")
	require.NoError(t, err)
	assert.Equal(t, uint32(42), n)

	require.NoError(t, foo.Foo(ctx, client, "foo"))

	for ok, want := range map[bool]*wrpc.Result[bool, string]{
		true:  wrpc.Ok[string](true),
		false: wrpc.Err[bool]("test"),
	} {
		v, err := sync.Fallible(ctx, client, ok)
		require.NoError(t, err)
		assert.Equal(t, want, v)
	}

	v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, err := sync.Numbers(ctx, client)
	require.NoError(t, err)
	assert.Equal(t,
		[]any{uint8(1), uint16(2), uint32(3), uint64(4), int8(5), int16(6), int32(7), int64(8), float32(9), float64(10)},
		[]any{v0, v1, v2, v3, v4, v5, v6, v7, v8, v9},
	)

	flags, err := sync.WithFlags(ctx, client, true, false, true)
	require.NoError(t, err)
	assert.Equal(t, &sync.Abc{A: true, C: true}, flags)

	rec := func(foo string) *sync.Rec {
		return &sync.Rec{Nested: &sync.RecNested{Foo: foo}}
	}

	opt, err := sync.WithVariantOption(ctx, client, true)
	require.NoError(t, err)
	assert.Equal(t, sync.NewVarVar(rec("bar")), opt)

	opt, err = sync.WithVariantOption(ctx, client, false)
	require.NoError(t, err)
	assert.Nil(t, opt)

	vars, err := sync.WithVariantList(ctx, client)
	require.NoError(t, err)
	empty := sync.NewVarEmpty()
	assert.Equal(t, []*sync.Var{empty, sync.NewVarVar(rec("foo")), empty, empty, empty, sync.NewVarVar(rec("bar"))}, vars)

	r, err := sync.WithRecord(ctx, client)
	require.NoError(t, err)
	assert.Equal(t, rec("foo"), r)

	rs, err := sync.WithRecordList(ctx, client, 3)
	require.NoError(t, err)
	assert.Equal(t, []*sync.Rec{rec("0"), rec("1"), rec("2")}, rs)
}

func TestFlags(t *testing.T) {
	for _, tc := range []struct {
		data string
		want *sync.Abc
	}{
		{data: "\x05", want: &sync.Abc{A: true, C: true}},
		{data: "\x08"},
		{data: "\x80"},
	} {
		ctx, client := serve(t, func(s wrpc.Server) (func() error, error) {
			return s.Serve("wrpc-test:integration/sync", "with-flags", func(ctx context.Context, w wrpc.IndexWriteCloser, r wrpc.IndexReadCloser) {
				_, _ = w.Write([]byte(tc.data))
				_ = w.Close()
			})
		})
		flags, err := sync.WithFlags(ctx, client, true, false, true)
		if tc.want == nil {
			assert.ErrorContains(t, err, "bit not associated with any flag is set", "%q", tc.data)
			assert.Nil(t, flags, "%q", tc.data)
			continue
		}
		require.NoError(t, err)
		assert.Equal(t, tc.want, flags)
	}
}

type floatsHandler struct {
	integration.SyncHandler
	floats chan [2]uint64
}

func (h floatsHandler) Numbers(ctx context.Context) (uint8, uint16, uint32, uint64, int8, int16, int32, int64, float32, float64, error) {
	select {
	case bits := <-h.floats:
		return 0, 0, 0, 0, 0, 0, 0, 0, math.Float32frombits(uint32(bits[0])), math.Float64frombits(bits[1]), nil
	case <-ctx.Done():
		return 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, ctx.Err()
	}
}

func TestCanonicalNaN(t *testing.T) {
	h := floatsHandler{floats: make(chan [2]uint64, 1)}
	ctx, client := serve(t, func(s wrpc.Server) (func() error, error) {
		return sync_server.Serve(s, h, h)
	})
	const nan32, nan64 = 0x7fc0_0000, 0x7ff8_0000_0000_0000
	for _, tc := range []struct {
		bits [2]uint64
		nan  bool
	}{
		{bits: [2]uint64{0x7f80_0001, 0x7ff0_0000_0000_0001}, nan: true},
		{bits: [2]uint64{0x7fa0_0000, 0x7ff4_0000_0000_0000}, nan: true},
		{bits: [2]uint64{0x7fc0_0001, 0x7ff8_0000_0000_0001}, nan: true},
		{bits: [2]uint64{0x7fff_ffff, 0x7fff_ffff_ffff_ffff}, nan: true},
		{bits: [2]uint64{0xff80_0001, 0xfff0_0000_0000_0001}, nan: true},
		{bits: [2]uint64{0xffc0_0000, 0xfff8_0000_0000_0000}, nan: true},
		{bits: [2]uint64{0xffff_ffff, 0xffff_ffff_ffff_ffff}, nan: true},
		{bits: [2]uint64{0x0000_0000, 0x0000_0000_0000_0000}},
		{bits: [2]uint64{0x8000_0000, 0x8000_0000_0000_0000}},
		{bits: [2]uint64{0x3fc0_0000, 0xbff8_0000_0000_0000}},
		{bits: [2]uint64{0x0000_0001, 0x0000_0000_0000_0001}},
		{bits: [2]uint64{0x7f7f_ffff, 0x7fef_ffff_ffff_ffff}},
		{bits: [2]uint64{0x7f80_0000, 0xfff0_0000_0000_0000}},
	} {
		bits, want := tc.bits, tc.bits
		if tc.nan {
			want = [2]uint64{nan32, nan64}
		}
		h.floats <- bits
		_, _, _, _, _, _, _, _, a, b, err := sync.Numbers(ctx, client)
		require.NoError(t, err)
		assert.Equal(t, want, [2]uint64{uint64(math.Float32bits(a)), math.Float64bits(b)}, "%#x", bits)
		assert.Equal(t,
			binary.LittleEndian.AppendUint64(binary.LittleEndian.AppendUint32(nil, uint32(want[0])), want[1]),
			wrpc.AppendFloat64(wrpc.AppendFloat32(nil, math.Float32frombits(uint32(bits[0]))), math.Float64frombits(bits[1])),
			"%#x", bits,
		)
	}
}

func TestSignedLEB128(t *testing.T) {
	for _, tc := range []struct {
		data string
		want [3]int64
		err  string
	}{
		{data: "\x80\x80\x7e" + "\x80\x80\x80\x80\x78" + "\x80\x80\x80\x80\x80\x80\x80\x80\x80\x7f", want: [3]int64{math.MinInt16, math.MinInt32, math.MinInt64}},
		{data: "\xff\xff\x01" + "\xff\xff\xff\xff\x07" + "\xff\xff\xff\xff\xff\xff\xff\xff\xff\x00", want: [3]int64{math.MaxInt16, math.MaxInt32, math.MaxInt64}},
		{data: "\x80\x80\x7a" + "\x00" + "\x00", err: "s16 overflows"},
		{data: "\x00" + "\x80\x80\x80\x80\x08" + "\x00", err: "s32 overflows"},
		{data: "\x00" + "\x00" + "\x80\x80\x80\x80\x80\x80\x80\x80\x80\x01", err: "s64 overflows"},
	} {
		ctx, client := serve(t, func(s wrpc.Server) (func() error, error) {
			return s.Serve("wrpc-test:integration/sync", "numbers", func(ctx context.Context, w wrpc.IndexWriteCloser, r wrpc.IndexReadCloser) {
				_, _ = w.Write([]byte("\x00\x00\x00\x00\x00" + tc.data + strings.Repeat("\x00", 12)))
				_ = w.Close()
			})
		})
		_, _, _, _, _, a, b, c, _, _, err := sync.Numbers(ctx, client)
		if tc.err != "" {
			assert.ErrorContains(t, err, tc.err, "%q", tc.data)
			continue
		}
		require.NoError(t, err)
		assert.Equal(t, tc.want, [3]int64{int64(a), int64(b), c})
	}
}
