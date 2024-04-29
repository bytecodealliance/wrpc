package types

import (
	"context"
	"fmt"
	"log/slog"

	wrpc "github.com/wrpc/wrpc/go"
)

type DiscriminantMethod uint32

const (
	DiscriminantMethod_Get DiscriminantMethod = iota
	DiscriminantMethod_Head
	DiscriminantMethod_Post
	DiscriminantMethod_Put
	DiscriminantMethod_Delete
	DiscriminantMethod_Connect
	DiscriminantMethod_Options
	DiscriminantMethod_Trace
	DiscriminantMethod_Patch
	DiscriminantMethod_Other
)

type VariantMethod struct {
	payload      any
	discriminant DiscriminantMethod
}

func NewMethod_Get() *VariantMethod {
	return &VariantMethod{nil, DiscriminantMethod_Get}
}

func NewMethod_Head() *VariantMethod {
	return &VariantMethod{nil, DiscriminantMethod_Head}
}

func NewMethod_Post() *VariantMethod {
	return &VariantMethod{nil, DiscriminantMethod_Post}
}

func NewMethod_Delete() *VariantMethod {
	return &VariantMethod{nil, DiscriminantMethod_Delete}
}

func NewMethod_Connect() *VariantMethod {
	return &VariantMethod{nil, DiscriminantMethod_Connect}
}

func NewMethod_Options() *VariantMethod {
	return &VariantMethod{nil, DiscriminantMethod_Options}
}

func NewMethod_Trace() *VariantMethod {
	return &VariantMethod{nil, DiscriminantMethod_Trace}
}

func NewMethod_Patch() *VariantMethod {
	return &VariantMethod{nil, DiscriminantMethod_Patch}
}

func NewMethod_Other(payload string) *VariantMethod {
	return &VariantMethod{payload, DiscriminantMethod_Other}
}

func (v *VariantMethod) Discriminant() DiscriminantMethod {
	return v.discriminant
}

func (v *VariantMethod) GetOther() (string, bool) {
	if v.discriminant != DiscriminantMethod_Other {
		return "", false
	}
	p, ok := v.payload.(string)
	return p, ok
}

func (v *VariantMethod) String() string {
	switch v.discriminant {
	case DiscriminantMethod_Get:
		return "get"
	case DiscriminantMethod_Head:
		return "head"
	case DiscriminantMethod_Post:
		return "post"
	case DiscriminantMethod_Delete:
		return "delete"
	case DiscriminantMethod_Connect:
		return "connect"
	case DiscriminantMethod_Options:
		return "options"
	case DiscriminantMethod_Trace:
		return "trace"
	case DiscriminantMethod_Patch:
		return "patch"
	case DiscriminantMethod_Other:
		return v.payload.(string)
	default:
		panic("unreachable")
	}
}

func ReadMethod(r wrpc.ByteReader) (*VariantMethod, error) {
	disc, err := wrpc.ReadUint32(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read `method` discriminant: %w", err)
	}
	switch DiscriminantMethod(disc) {
	case DiscriminantMethod_Get:
		return &VariantMethod{nil, DiscriminantMethod_Get}, nil
	case DiscriminantMethod_Head:
		return &VariantMethod{nil, DiscriminantMethod_Head}, nil
	case DiscriminantMethod_Post:
		return &VariantMethod{nil, DiscriminantMethod_Post}, nil
	case DiscriminantMethod_Delete:
		return &VariantMethod{nil, DiscriminantMethod_Delete}, nil
	case DiscriminantMethod_Connect:
		return &VariantMethod{nil, DiscriminantMethod_Connect}, nil
	case DiscriminantMethod_Options:
		return &VariantMethod{nil, DiscriminantMethod_Options}, nil
	case DiscriminantMethod_Trace:
		return &VariantMethod{nil, DiscriminantMethod_Trace}, nil
	case DiscriminantMethod_Patch:
		return &VariantMethod{nil, DiscriminantMethod_Patch}, nil
	case DiscriminantMethod_Other:
		payload, err := wrpc.ReadString(r)
		if err != nil {
			return nil, fmt.Errorf("failed to read `method::other` value: %w", err)
		}
		return &VariantMethod{payload, DiscriminantMethod_Other}, nil
	default:
		return nil, fmt.Errorf("unknown `method` discriminant value %d", disc)
	}
}

type DiscriminantScheme uint32

const (
	DiscriminantScheme_Http DiscriminantScheme = iota
	DiscriminantScheme_Https
	DiscriminantScheme_Other
)

type VariantScheme struct {
	payload      any
	discriminant DiscriminantScheme
}

func NewScheme_Http() *VariantScheme {
	return &VariantScheme{nil, DiscriminantScheme_Http}
}

func NewScheme_Https() *VariantScheme {
	return &VariantScheme{nil, DiscriminantScheme_Https}
}

func NewScheme_Other(payload string) *VariantScheme {
	return &VariantScheme{payload, DiscriminantScheme_Other}
}

func (v *VariantScheme) Discriminant() DiscriminantScheme {
	return v.discriminant
}

func (v *VariantScheme) GetOther() (string, bool) {
	if v.discriminant != DiscriminantScheme_Other {
		return "", false
	}
	p, ok := v.payload.(string)
	return p, ok
}

func (v *VariantScheme) String() string {
	switch v.discriminant {
	case DiscriminantScheme_Http:
		return "http"
	case DiscriminantScheme_Https:
		return "https"
	case DiscriminantScheme_Other:
		return v.payload.(string)
	default:
		panic("unreachable")
	}
}

func ReadScheme(r wrpc.ByteReader) (*VariantScheme, error) {
	disc, err := wrpc.ReadUint32(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read `scheme` discriminant: %w", err)
	}
	switch DiscriminantScheme(disc) {
	case DiscriminantScheme_Http:
		return &VariantScheme{nil, DiscriminantScheme_Http}, nil
	case DiscriminantScheme_Https:
		return &VariantScheme{nil, DiscriminantScheme_Https}, nil
	case DiscriminantScheme_Other:
		payload, err := wrpc.ReadString(r)
		if err != nil {
			return nil, fmt.Errorf("failed to read `scheme::other` value: %w", err)
		}
		return &VariantScheme{payload, DiscriminantScheme_Other}, nil
	default:
		return nil, fmt.Errorf("unknown `scheme` discriminant value %d", disc)
	}
}

type (
	SubscriptionRequest struct {
		payloadBody     <-chan []byte
		payloadTrailers <-chan []byte

		stopBody     func() error
		stopTrailers func() error
	}
	RecordRequest struct {
		Body          wrpc.ReadyReader
		Trailers      wrpc.ReadyReceiver[[]*wrpc.Tuple2[string, [][]byte]]
		Method        *VariantMethod
		PathWithQuery *string
		Scheme        *VariantScheme
		Authority     *string
		Headers       []*wrpc.Tuple2[string, [][]byte]
	}

	SubscriptionRequestOptions struct{}
	RecordRequestOptions       struct {
		ConnectTimeout     *uint64
		FirstByteTimeout   *uint64
		BetweenByteTimeout *uint64
	}

	SubscriptionResponse struct {
		payload         <-chan []byte
		payloadBody     <-chan []byte
		payloadTrailers <-chan []byte

		stop         func() error
		stopBody     func() error
		stopTrailers func() error

		errorsBody     <-chan error
		errorsTrailers <-chan error

		buffer []byte
	}
	RecordResponse struct {
		Body     wrpc.ReadyReader
		Trailers wrpc.ReadyReceiver[[]*wrpc.Tuple2[string, [][]byte]]
		Status   uint16
		Headers  []*wrpc.Tuple2[string, [][]byte]
	}
)

func SubscribeRequest(sub wrpc.Subscriber) (*SubscriptionRequest, error) {
	slog.Debug("subscribe for `body`")
	payloadBody := make(chan []byte)
	stopBody, err := sub.SubscribePath([]uint32{0}, func(ctx context.Context, buf []byte) {
		payloadBody <- buf
	})
	if err != nil {
		return nil, err
	}

	slog.Debug("subscribe for `trailers`")
	payloadTrailers := make(chan []byte)
	stopTrailers, err := sub.SubscribePath([]uint32{1}, func(ctx context.Context, buf []byte) {
		payloadTrailers <- buf
	})
	if err != nil {
		defer func() {
			if err := stopBody(); err != nil {
				slog.Error("failed to stop `body` subscription", "err", err)
			}
		}()
		return nil, err
	}

	return &SubscriptionRequest{
		payloadBody,
		payloadTrailers,
		stopBody,
		stopTrailers,
	}, nil
}

func ReadRequest(ctx context.Context, r wrpc.ByteReader, sub *SubscriptionRequest) (*RecordRequest, error) {
	slog.DebugContext(ctx, "reading `body`")
	body, err := wrpc.ReadByteStream(ctx, r, sub.payloadBody)
	if err != nil {
		return nil, fmt.Errorf("failed to read `body`: %w", err)
	}
	slog.DebugContext(ctx, "read `body`", "body", body)

	slog.DebugContext(ctx, "reading `trailers`")
	trailers, err := wrpc.ReadFuture(ctx, r, sub.payloadTrailers, func(r wrpc.ByteReader) ([]*wrpc.Tuple2[string, [][]byte], error) {
		return wrpc.ReadList(r, func(r wrpc.ByteReader) (*wrpc.Tuple2[string, [][]byte], error) {
			return wrpc.ReadTuple2(r, wrpc.ReadString, func(r wrpc.ByteReader) ([][]byte, error) {
				return wrpc.ReadList(r, wrpc.ReadByteList)
			})
		})
	})
	if err != nil {
		return nil, fmt.Errorf("failed to read `trailers`: %w", err)
	}
	slog.DebugContext(ctx, "read `trailers`", "trailers", trailers)

	slog.DebugContext(ctx, "reading `method`")
	method, err := ReadMethod(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read `method`: %w", err)
	}
	slog.DebugContext(ctx, "read `method`", "method", method)

	slog.DebugContext(ctx, "reading `path-with-query`")
	pathWithQuery, err := wrpc.ReadOption(r, wrpc.ReadString)
	if err != nil {
		return nil, fmt.Errorf("failed to read `path-with-query`: %w", err)
	}
	slog.DebugContext(ctx, "read `path-with-query`", "path-with-query", *pathWithQuery)

	slog.DebugContext(ctx, "reading `scheme`")
	scheme, err := wrpc.ReadFlatOption(r, ReadScheme)
	if err != nil {
		return nil, fmt.Errorf("failed to read `scheme`: %w", err)
	}
	slog.DebugContext(ctx, "read `scheme`", "scheme", scheme)

	slog.DebugContext(ctx, "reading `authority`")
	authority, err := wrpc.ReadOption(r, wrpc.ReadString)
	if err != nil {
		return nil, fmt.Errorf("failed to read `authority`: %w", err)
	}
	slog.DebugContext(ctx, "read `authority`", "authority", authority)

	slog.DebugContext(ctx, "reading `headers`")
	headers, err := wrpc.ReadList(r, func(r wrpc.ByteReader) (*wrpc.Tuple2[string, [][]byte], error) {
		return wrpc.ReadTuple2(r, wrpc.ReadString, func(r wrpc.ByteReader) ([][]byte, error) {
			return wrpc.ReadList(r, wrpc.ReadByteList)
		})
	})
	if err != nil {
		return nil, fmt.Errorf("failed to read `headers`: %w", err)
	}
	slog.DebugContext(ctx, "read `headers`", "headers", headers)

	return &RecordRequest{body, trailers, method, pathWithQuery, scheme, authority, headers}, nil
}

func ReadRequestOptions(r wrpc.ByteReader) (*RecordRequestOptions, error) {
	slog.Debug("reading `connect-timeout`")
	connectTimeout, err := wrpc.ReadOption(r, wrpc.ReadUint64)
	if err != nil {
		return nil, fmt.Errorf("failed to read `connect-timeout`: %w", err)
	}

	slog.Debug("reading `first-byte-timeout`")
	firstByteTimeout, err := wrpc.ReadOption(r, wrpc.ReadUint64)
	if err != nil {
		return nil, fmt.Errorf("failed to read `first-byte-timeout`: %w", err)
	}

	slog.Debug("reading `between-byte-timeout`")
	betweenByteTimeout, err := wrpc.ReadOption(r, wrpc.ReadUint64)
	if err != nil {
		return nil, fmt.Errorf("failed to read `between-byte-timeout`: %w", err)
	}

	return &RecordRequestOptions{connectTimeout, firstByteTimeout, betweenByteTimeout}, nil
}
