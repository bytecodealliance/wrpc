//go:generate $WIT_BINDGEN_WRPC go --gofmt=false --world sync-client --out-dir bindings/sync_client --package wrpc.io/tests/go/bindings/sync_client ../wit
//go:generate $WIT_BINDGEN_WRPC go --gofmt=false --world runner --out-dir bindings/maps --package wrpc.io/tests/go/bindings/maps ../runtime/map/test.wit

package integration_test

import (
	"context"
	"encoding/binary"
	"io"
	"math"
	"strings"
	"testing"
	"time"

	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
	wrpc "wrpc.io/go"
	integration "wrpc.io/tests/go"
	"wrpc.io/tests/go/bindings/maps/test/maps/to_test"
	"wrpc.io/tests/go/bindings/sync_client"
	"wrpc.io/tests/go/bindings/sync_client/foo"
	"wrpc.io/tests/go/bindings/sync_client/strange"
	"wrpc.io/tests/go/bindings/sync_client/wrpc_test/integration/resources"
	"wrpc.io/tests/go/bindings/sync_client/wrpc_test/integration/sync"
	"wrpc.io/tests/go/bindings/sync_server"
	sync_server_foo "wrpc.io/tests/go/bindings/sync_server/exports/foo"
	sync_server_sync "wrpc.io/tests/go/bindings/sync_server/exports/wrpc_test/integration/sync"
	"wrpc.io/tests/go/internal"
)

func serveSync(t *testing.T, h interface {
	sync_server_sync.Handler
	sync_server_foo.Handler
	sync_server.Handler
}) (context.Context, func() *wrpc.Framed) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	t.Cleanup(cancel)
	rh := &integration.ResourcesHandler{}
	return ctx, internal.Serve(t, ctx, func(ctx context.Context, hdr wrpc.Header, c *wrpc.Framed) (bool, error) {
		return sync_server.Handle(ctx, hdr, c, h, h, h, rh, integration.StrangeHandler{Resources: rh})
	})
}

func serveRaw(t *testing.T, data string) (context.Context, *wrpc.Framed) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	t.Cleanup(cancel)
	connect := internal.Serve(t, ctx, func(ctx context.Context, hdr wrpc.Header, c *wrpc.Framed) (bool, error) {
		for {
			path, n, err := c.R.Next()
			if err != nil {
				return true, err
			}
			if _, err := io.Copy(io.Discard, c.R); err != nil {
				return true, err
			}
			if len(path) == 0 && n == 0 {
				break
			}
		}
		if err := c.W.WriteFrame(nil, []byte(data)); err != nil {
			return true, err
		}
		return true, c.W.WriteFrame(nil, nil)
	})
	return ctx, connect()
}

func TestSync(t *testing.T) {
	ctx, connect := serveSync(t, integration.SyncHandler{})
	client := connect()

	n, err := foo.F(ctx, client, "f")
	require.NoError(t, err)
	assert.Equal(t, uint32(42), n)

	require.NoError(t, foo.Foo(ctx, client, "foo"))
	require.Error(t, foo.Foo(ctx, client, "bar"))
	client = connect()

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

	pong, err := sync_client.Ping(ctx, client)
	require.NoError(t, err)
	assert.Equal(t, "pong", pong)

	res, err := resources.NewFoo(ctx, client)
	require.NoError(t, err)

	v, err := resources.Foo_Bar(ctx, client, res.Borrow())
	require.NoError(t, err)
	assert.Equal(t, "bar", v)

	v, err = resources.Bar(ctx, client, res.Borrow())
	require.NoError(t, err)
	assert.Equal(t, "bar", v)

	n64, err := strange.Bar(ctx, client, res.Borrow())
	require.NoError(t, err)
	assert.Equal(t, uint64(42), n64)

	v, err = resources.Foo_Foo(ctx, client, res)
	require.NoError(t, err)
	assert.Equal(t, "foo", v)

	_, err = resources.Foo_Bar(ctx, client, res.Borrow())
	assert.Error(t, err)
	client = connect()

	a, err := resources.NewCounter(ctx, client, 1)
	require.NoError(t, err)

	require.NoError(t, resources.Counter_IncrementBy(ctx, client, a.Borrow(), 2))

	n32, err := resources.Counter_GetCount(ctx, client, a.Borrow())
	require.NoError(t, err)
	assert.Equal(t, uint32(3), n32)

	b, err := resources.Counter_CloneCounter(ctx, client, a.Borrow())
	require.NoError(t, err)

	require.NoError(t, resources.Counter_IncrementBy(ctx, client, b.Borrow(), 1))

	n32, err = resources.Counter_Sum(ctx, client, a.Borrow(), b.Borrow())
	require.NoError(t, err)
	assert.Equal(t, uint32(7), n32)

	// TODO: Test dropping `foo`, also after moving it, once resource dropping is supported
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
		ctx, client := serveRaw(t, tc.data)
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

func TestMapDuplicateKey(t *testing.T) {
	ctx, client := serveRaw(t, "\x02\x01\x01a\x01\x01b")
	m, err := to_test.EmptyRoundtrip(ctx, client, nil)
	assert.ErrorContains(t, err, "duplicate key at entry 1")
	assert.Nil(t, m)
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
	ctx, connect := serveSync(t, h)
	client := connect()
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
		{data: "\x80\x80\x7a" + "\x00" + "\x00", err: "signed integer overflow"},
		{data: "\x00" + "\x80\x80\x80\x80\x08" + "\x00", err: "signed integer overflow"},
		{data: "\x00" + "\x00" + "\x80\x80\x80\x80\x80\x80\x80\x80\x80\x01", err: "signed integer overflow"},
	} {
		ctx, client := serveRaw(t, "\x00\x00\x00\x00\x00"+tc.data+strings.Repeat("\x00", 12))
		_, _, _, _, _, a, b, c, _, _, err := sync.Numbers(ctx, client)
		if tc.err != "" {
			assert.ErrorContains(t, err, tc.err, "%q", tc.data)
			continue
		}
		require.NoError(t, err)
		assert.Equal(t, tc.want, [3]int64{int64(a), int64(b), c})
	}
}
