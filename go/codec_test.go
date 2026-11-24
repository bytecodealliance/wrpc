package wrpc_test

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"math"
	"net"
	"reflect"
	"testing"
	"time"

	wrpc "wrpc.io/go"
)

type testRecord struct {
	A uint32
	S <-chan []<-chan string
}

type testRecordCodec struct{}

var testRecordStreamCodec = wrpc.StreamCodec[<-chan string]{Elem: wrpc.FutureCodec[string]{Elem: wrpc.StringCodec{}}}

func (testRecordCodec) Encode(v *testRecord, dst []byte) ([]byte, wrpc.Transmitter, error) {
	dst, _, err := wrpc.U32Codec{}.Encode(v.A, dst)
	if err != nil {
		return nil, nil, err
	}
	dst, tx, err := testRecordStreamCodec.Encode(v.S, dst)
	if err != nil || tx == nil {
		return dst, nil, err
	}
	return dst, wrpc.Transmitters{1: tx}, nil
}

func (testRecordCodec) Decode(r wrpc.ByteReader) (*testRecord, wrpc.Receiver, error) {
	v := &testRecord{}
	var err error
	if v.A, _, err = (wrpc.U32Codec{}).Decode(r); err != nil {
		return nil, nil, err
	}
	var rx wrpc.Receiver
	if v.S, rx, err = testRecordStreamCodec.Decode(r); err != nil || rx == nil {
		return v, nil, err
	}
	return v, wrpc.Receivers{1: rx}, nil
}

type testVariant struct {
	Discriminant uint8
	A            <-chan []string
	B            uint8
}

type testVariantCodec struct{}

var testVariantStreamCodec = wrpc.StreamCodec[string]{Elem: wrpc.StringCodec{}}

func (testVariantCodec) Encode(v *testVariant, dst []byte) ([]byte, wrpc.Transmitter, error) {
	dst = wrpc.AppendUint32(dst, uint32(v.Discriminant))
	switch v.Discriminant {
	case 0:
		return testVariantStreamCodec.Encode(v.A, dst)
	case 1:
		return wrpc.U8Codec{}.Encode(v.B, dst)
	case 2:
		return dst, nil, nil
	default:
		return nil, nil, errors.New("invalid discriminant")
	}
}

func (testVariantCodec) Decode(r wrpc.ByteReader) (*testVariant, wrpc.Receiver, error) {
	d, err := wrpc.ReadUint32(r)
	if err != nil {
		return nil, nil, err
	}
	v := &testVariant{Discriminant: uint8(d)}
	var rx wrpc.Receiver
	switch d {
	case 0:
		v.A, rx, err = testVariantStreamCodec.Decode(r)
	case 1:
		v.B, rx, err = wrpc.U8Codec{}.Decode(r)
	case 2:
	default:
		err = fmt.Errorf("invalid discriminant `%d`", d)
	}
	if err != nil {
		return nil, nil, err
	}
	return v, rx, nil
}

type testFixedListCodec struct{}

var testFixedListElemCodec = wrpc.FutureCodec[uint8]{Elem: wrpc.U8Codec{}}

func (testFixedListCodec) Encode(v [2]<-chan uint8, dst []byte) ([]byte, wrpc.Transmitter, error) {
	return wrpc.EncodeFixedList(testFixedListElemCodec, v[:], dst)
}

func (testFixedListCodec) Decode(r wrpc.ByteReader) ([2]<-chan uint8, wrpc.Receiver, error) {
	var v [2]<-chan uint8
	rx, err := wrpc.DecodeFixedList(testFixedListElemCodec, r, v[:])
	return v, rx, err
}

func pendingFuture[T any](release <-chan struct{}, v T) <-chan T {
	ch := make(chan T, 1)
	go func() {
		<-release
		ch <- v
		close(ch)
	}()
	return ch
}

func readyFuture[T any](v T) <-chan T {
	ch := make(chan T, 1)
	ch <- v
	close(ch)
	return ch
}

func pendingStream[T any](release <-chan struct{}, chunks ...[]T) <-chan []T {
	ch := make(chan []T)
	go func() {
		<-release
		for _, c := range chunks {
			ch <- c
		}
		close(ch)
	}()
	return ch
}

func collectStream[T, U any](s <-chan []T, f func(T) U) []U {
	var vs []U
	for chunk := range s {
		for _, v := range chunk {
			vs = append(vs, f(v))
		}
	}
	return vs
}

func recv[T any](ch <-chan T) T {
	return <-ch
}

func testRoundTrip[T any](t *testing.T, c wrpc.Codec[T], v func(release <-chan struct{}) T, collect func(T) any, want any) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	tc := wrpc.Tuple1Codec[T]{C0: c}
	clt, srv := net.Pipe()
	defer clt.Close()
	srvErr := make(chan error, 1)
	go func() {
		defer srv.Close()
		_, c, err := wrpc.NewListener(srv, srv).Accept(ctx)
		if err != nil {
			srvErr <- err
			return
		}
		srvErr <- wrpc.Serve(ctx, c, tc, tc, func(_ context.Context, p wrpc.Tuple1[T]) (wrpc.Tuple1[T], error) {
			return p, nil
		})
	}()

	release := make(chan struct{})
	res, inv, err := wrpc.Invoke(ctx, wrpc.NewFramed(clt, clt), "test", "identity", tc, tc, wrpc.Tuple1[T]{V0: v(release)})
	close(release)
	if err != nil {
		t.Fatal(err)
	}
	got := make(chan any, 1)
	go func() { got <- collect(res.V0) }()
	if err := inv.Wait(); err != nil {
		t.Fatal(err)
	}
	if err := <-srvErr; err != nil {
		t.Fatal(err)
	}
	select {
	case got := <-got:
		if !reflect.DeepEqual(got, want) {
			t.Fatalf("got %#v, want %#v", got, want)
		}
	case <-ctx.Done():
		t.Fatal(ctx.Err())
	}
}

func TestCodecRoundTrip(t *testing.T) {
	t.Run("primitives", func(t *testing.T) {
		type prims = wrpc.Tuple16[bool, uint8, uint16, uint32, uint64, int8, int16, int32, int64, float32, float64, rune, string, []byte, wrpc.Own[struct{}], uint32]
		v := prims{true, math.MaxUint8, math.MaxUint16, math.MaxUint32, math.MaxUint64, math.MinInt8, math.MinInt16, -1, math.MinInt64, 1.5, -2.5, '🦀', "foo", []byte("bar"), wrpc.Own[struct{}]("baz"), 0b101}
		c := wrpc.Tuple16Codec[bool, uint8, uint16, uint32, uint64, int8, int16, int32, int64, float32, float64, rune, string, []byte, wrpc.Own[struct{}], uint32]{
			wrpc.BoolCodec{}, wrpc.U8Codec{}, wrpc.U16Codec{}, wrpc.U32Codec{}, wrpc.U64Codec{},
			wrpc.S8Codec{}, wrpc.S16Codec{}, wrpc.S32Codec{}, wrpc.S64Codec{}, wrpc.F32Codec{}, wrpc.F64Codec{},
			wrpc.CharCodec{}, wrpc.StringCodec{}, wrpc.BytesCodec{}, wrpc.OwnCodec[struct{}]{}, wrpc.FlagsCodec{N: 3},
		}
		testRoundTrip(t, c, func(<-chan struct{}) prims { return v }, func(v prims) any { return v }, v)
	})
	t.Run("option and result", func(t *testing.T) {
		type opts = wrpc.Tuple4[*string, *wrpc.Result[struct{}, string], *<-chan string, *wrpc.Result[<-chan []byte, struct{}]]
		c := wrpc.Tuple4Codec[*string, *wrpc.Result[struct{}, string], *<-chan string, *wrpc.Result[<-chan []byte, struct{}]]{
			wrpc.OptionCodec[string]{Elem: wrpc.StringCodec{}},
			wrpc.ResultCodec[struct{}, string]{Ok: wrpc.UnitCodec{}, Err: wrpc.StringCodec{}},
			wrpc.OptionCodec[<-chan string]{Elem: wrpc.FutureCodec[string]{Elem: wrpc.StringCodec{}}},
			wrpc.ResultCodec[<-chan []byte, struct{}]{Ok: wrpc.ByteStreamCodec{}, Err: wrpc.UnitCodec{}},
		}
		testRoundTrip(t, c, func(release <-chan struct{}) opts {
			f := pendingFuture(release, "foo")
			return opts{nil, wrpc.Err[struct{}]("bar"), &f, wrpc.Ok[struct{}](pendingStream(release, []byte("ba"), []byte("z")))}
		}, func(v opts) any {
			return []any{v.V0, *v.V1.Err, collectStream(*v.V3.Ok, func(b byte) byte { return b }), recv(*v.V2)}
		}, []any{(*string)(nil), "bar", []byte("baz"), "foo"})
	})
	t.Run("list of futures", func(t *testing.T) {
		testRoundTrip(t, wrpc.ListCodec[<-chan string]{Elem: wrpc.FutureCodec[string]{Elem: wrpc.StringCodec{}}}, func(release <-chan struct{}) []<-chan string {
			return []<-chan string{readyFuture("foo"), pendingFuture(release, "bar")}
		}, func(v []<-chan string) any {
			return []string{<-v[0], <-v[1]}
		}, []string{"foo", "bar"})
	})
	t.Run("record with stream of futures", func(t *testing.T) {
		testRoundTrip(t, testRecordCodec{}, func(release <-chan struct{}) *testRecord {
			return &testRecord{A: 42, S: pendingStream(release, []<-chan string{pendingFuture(release, "foo"), readyFuture("bar")}, []<-chan string{pendingFuture(release, "baz")})}
		}, func(v *testRecord) any {
			return []any{v.A, collectStream(v.S, recv[string])}
		}, []any{uint32(42), []string{"foo", "bar", "baz"}})
	})
	t.Run("variant payloads", func(t *testing.T) {
		testRoundTrip(t, wrpc.ListCodec[*testVariant]{Elem: testVariantCodec{}}, func(release <-chan struct{}) []*testVariant {
			return []*testVariant{{Discriminant: 1, B: 42}, {Discriminant: 0, A: pendingStream(release, []string{"foo", "bar"})}, {Discriminant: 2}}
		}, func(v []*testVariant) any {
			return []any{v[0].B, collectStream(v[1].A, func(s string) string { return s }), v[2].Discriminant}
		}, []any{uint8(42), []string{"foo", "bar"}, uint8(2)})
	})
	t.Run("nested futures", func(t *testing.T) {
		c := wrpc.FutureCodec[<-chan <-chan []string]{Elem: wrpc.FutureCodec[<-chan []string]{Elem: wrpc.StreamCodec[string]{Elem: wrpc.StringCodec{}}}}
		testRoundTrip(t, c, func(release <-chan struct{}) <-chan <-chan <-chan []string {
			return pendingFuture(release, pendingFuture(release, pendingStream(release, []string{"foo"}, []string{"bar"})))
		}, func(v <-chan <-chan <-chan []string) any {
			return collectStream(<-<-v, func(s string) string { return s })
		}, []string{"foo", "bar"})
	})
	t.Run("map values", func(t *testing.T) {
		testRoundTrip(t, wrpc.MapCodec[string, <-chan string]{Key: wrpc.StringCodec{}, Value: wrpc.FutureCodec[string]{Elem: wrpc.StringCodec{}}}, func(release <-chan struct{}) map[string]<-chan string {
			return map[string]<-chan string{"a": pendingFuture(release, "foo"), "b": pendingFuture(release, "bar")}
		}, func(v map[string]<-chan string) any {
			return map[string]string{"a": <-v["a"], "b": <-v["b"]}
		}, map[string]string{"a": "foo", "b": "bar"})
	})
	t.Run("fixed-size list", func(t *testing.T) {
		testRoundTrip(t, testFixedListCodec{}, func(release <-chan struct{}) [2]<-chan uint8 {
			return [2]<-chan uint8{readyFuture[uint8](1), pendingFuture[uint8](release, 2)}
		}, func(v [2]<-chan uint8) any {
			return [2]uint8{<-v[0], <-v[1]}
		}, [2]uint8{1, 2})
	})
	t.Run("unit stream and future", func(t *testing.T) {
		type units = wrpc.Tuple2[<-chan []struct{}, <-chan struct{}]
		c := wrpc.Tuple2Codec[<-chan []struct{}, <-chan struct{}]{wrpc.StreamCodec[struct{}]{Elem: wrpc.UnitCodec{}}, wrpc.FutureCodec[struct{}]{Elem: wrpc.UnitCodec{}}}
		testRoundTrip(t, c, func(release <-chan struct{}) units {
			return units{pendingStream(release, make([]struct{}, 2), make([]struct{}, 1)), pendingFuture(release, struct{}{})}
		}, func(v units) any {
			return []any{len(collectStream(v.V0, func(v struct{}) struct{} { return v })), recv(v.V1)}
		}, []any{3, struct{}{}})
	})
}

func receiveRoot[T any](c wrpc.Codec[T], data string) error {
	_, _, err := wrpc.Receive(wrpc.NewFrameReader(bytes.NewReader(appendFrames(nil, frame{nil, data}, frame{nil, ""}))), c)
	return err
}

func TestCodecRejectsInvalidValues(t *testing.T) {
	for _, err := range []error{
		receiveRoot(wrpc.BoolCodec{}, "\x02"),
		receiveRoot(wrpc.CharCodec{}, "\xff"),
		receiveRoot(wrpc.S16Codec{}, "\x80\x80\x02"),
		receiveRoot(wrpc.FlagsCodec{N: 3}, "\x08"),
		receiveRoot(wrpc.MapCodec[string, uint8]{Key: wrpc.StringCodec{}, Value: wrpc.U8Codec{}}, "\x02\x01a\x00\x01a\x01"),
	} {
		if err == nil {
			t.Fatal("invalid value decoded")
		}
	}
}
