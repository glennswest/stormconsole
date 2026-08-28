FROM registry.gt.lo:5000/stormdbase:latest
COPY stormconsole /app/stormconsole
COPY config/stormd.toml /etc/stormd/config.toml
COPY config/config.toml /etc/stormconsole/config.toml
EXPOSE 9080 9094 22
ENTRYPOINT ["/stormd"]
