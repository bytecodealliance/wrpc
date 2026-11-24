package wrpc_test

import (
	"bytes"
	"context"
	"errors"
	"slices"
	"testing"
	"time"

	wrpc "wrpc.io/go"
)

type frame struct {
	path []uint32
	data string
}

func appendFrames(dst []byte, fs ...frame) []byte {
	buf := bytes.NewBuffer(dst)
	w := wrpc.NewFrameWriter(buf)
	for _, f := range fs {
		if err := w.WriteFrame(f.path, []byte(f.data)); err != nil {
			panic(err)
		}
	}
	return buf.Bytes()
}

func receiveByteStream(ctx context.Context, r *wrpc.FrameReader) ([]byte, error) {
	v, rx, err := wrpc.Receive(r, wrpc.Tuple1Codec[<-chan []byte]{wrpc.ByteStreamCodec{}})
	if err != nil {
		return nil, err
	}
	errs := make(chan error, 1)
	go func() { errs <- rx.Wait(ctx) }()
	var buf []byte
	for {
		select {
		case b, ok := <-v.V0:
			if !ok {
				return buf, <-errs
			}
			buf = append(buf, b...)
		case err := <-errs:
			if err != nil {
				return nil, err
			}
			for b := range v.V0 {
				buf = append(buf, b...)
			}
			return buf, nil
		}
	}
}

func TestReceive(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	r := wrpc.NewFrameReader(bytes.NewReader(appendFrames(nil,
		frame{nil, "\x00"},
		frame{[]uint32{0}, "\x03foo"},
		frame{nil, ""},
		frame{[]uint32{0}, "\x03b"},
		frame{[]uint32{0}, "ar"},
		frame{[]uint32{0}, ""},
		frame{nil, "\x2a"},
	)))
	buf, err := receiveByteStream(ctx, r)
	if err != nil {
		t.Fatal(err)
	}
	if string(buf) != "foobar" {
		t.Fatalf("unexpected stream contents `%s`", buf)
	}
	path, n, err := r.Next()
	if err != nil || len(path) != 0 || n != 1 {
		t.Fatalf("unexpected frame after the invocation: %v %d %v", path, n, err)
	}
}

func TestReceiveRejectsInvalidFrames(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	root := slices.Clip(appendFrames(nil, frame{nil, "\x00"}, frame{nil, ""}))
	for _, tc := range []struct {
		frames []byte
		kind   wrpc.ReceiveErrorKind
	}{
		{appendFrames(root, frame{[]uint32{1}, ""}), wrpc.ReceiveUnknownPath},
		{appendFrames(nil, frame{nil, "\x00\x2a"}), wrpc.ReceiveDataAfterEnd},
		{appendFrames(nil, frame{nil, "\x02"}, frame{nil, ""}), wrpc.ReceiveDecode},
		{root, wrpc.ReceiveTruncated},
		{appendFrames(root, frame{[]uint32{0}, "\x03fo"}, frame{[]uint32{0}, ""}), wrpc.ReceiveDecode},
		{append(root, "\x01\x00\x05\x2a"...), wrpc.ReceiveFrame},
	} {
		_, err := receiveByteStream(ctx, wrpc.NewFrameReader(bytes.NewReader(tc.frames)))
		var rerr *wrpc.ReceiveError
		if !errors.As(err, &rerr) || rerr.Kind != tc.kind {
			t.Fatalf("unexpected error for %x: %v", tc.frames, err)
		}
	}
}

func TestReceiveStreamOfFutures(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	r := wrpc.NewFrameReader(bytes.NewReader(appendFrames(nil,
		frame{nil, "\x00"},
		frame{nil, ""},
		frame{[]uint32{0}, "\x01\x00"},
		frame{[]uint32{0}, "\x01\x00"},
		frame{[]uint32{0, 0}, "\x01a"},
		frame{[]uint32{0, 0}, ""},
		frame{[]uint32{0, 1}, "\x01b"},
		frame{[]uint32{0, 1}, ""},
		frame{[]uint32{0}, ""},
	)))
	v, rx, err := wrpc.Receive(r, wrpc.Tuple1Codec[<-chan []<-chan string]{
		wrpc.StreamCodec[<-chan string]{wrpc.FutureCodec[string]{wrpc.StringCodec{}}},
	})
	if err != nil {
		t.Fatal(err)
	}
	errs := make(chan error, 1)
	go func() { errs <- rx.Wait(ctx) }()
	for _, want := range []string{"a", "b"} {
		var chunk []<-chan string
		select {
		case chunk = <-v.V0:
		case <-ctx.Done():
			t.Fatal("stream chunk not received")
		}
		if len(chunk) != 1 {
			t.Fatalf("unexpected chunk length %d", len(chunk))
		}
		select {
		case got := <-chunk[0]:
			if got != want {
				t.Fatalf("unexpected future value `%s`", got)
			}
		case <-ctx.Done():
			t.Fatal("future value not received")
		}
	}
	if _, ok := <-v.V0; ok {
		t.Fatal("stream not closed")
	}
	if err := <-errs; err != nil {
		t.Fatal(err)
	}
}

func TestReadHeaderRejectsInvalidHeaders(t *testing.T) {
	for _, tc := range []struct {
		data string
		kind wrpc.HeaderDecodeErrorKind
	}{
		{"\x02", wrpc.HeaderUnsupportedVersion},
		{"\x01\x03fo", wrpc.HeaderTruncated},
		{"\x01\x01\xff", wrpc.HeaderInvalidName},
		{"\x01\xff\xff\xff\xff\x7f", wrpc.HeaderOverflow},
	} {
		_, err := wrpc.NewFrameReader(bytes.NewReader([]byte(tc.data))).ReadHeader()
		var herr *wrpc.HeaderDecodeError
		if !errors.As(err, &herr) || herr.Kind != tc.kind {
			t.Fatalf("unexpected error for %x: %v", tc.data, err)
		}
	}
}
