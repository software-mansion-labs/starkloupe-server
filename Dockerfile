FROM public.ecr.aws/docker/library/rust:1.74.1

WORKDIR /usr/src/myapp
COPY . .

ENV DATABASE_URL="postgresql://wido:Prankster-Wido@wido-1.cn5qetssppiq.us-east-1.rds.amazonaws.com:5432/walnut"
ENV REDIS_ADDR="redis://widoserver-east-1.h4j9ed.0001.use1.cache.amazonaws.com:6379"

RUN cargo build --release server

EXPOSE 3000

CMD ["./target/release/server"]
