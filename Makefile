LIB_NAME = libsubxt_uniffi
GEN_DIR = generated
FRAMEWORK = $(GEN_DIR)/SubxtUniFFI.xcframework
HEADERS_DIR = $(GEN_DIR)/headers

IOS_TARGET = aarch64-apple-ios
SIM_TARGET = aarch64-apple-ios-sim

IOS_LIB = target/$(IOS_TARGET)/release/$(LIB_NAME).a
SIM_LIB = target/$(SIM_TARGET)/release/$(LIB_NAME).a

.PHONY: setup build-ios build-sim bindings xcframework xcode all clean

setup:
	rustup target add $(IOS_TARGET) $(SIM_TARGET)

build-ios:
	cargo build --target $(IOS_TARGET) --release

build-sim:
	cargo build --target $(SIM_TARGET) --release

bindings: build-ios
	mkdir -p $(GEN_DIR)
	cargo run --manifest-path uniffi-bindgen/Cargo.toml -- \
		generate --library $(IOS_LIB) \
		--language swift --out-dir $(GEN_DIR)

xcframework: build-ios build-sim bindings
	rm -rf $(HEADERS_DIR) $(FRAMEWORK)
	mkdir -p $(HEADERS_DIR)
	cp $(GEN_DIR)/subxt_uniffiFFI.h $(HEADERS_DIR)/
	cp $(GEN_DIR)/subxt_uniffiFFI.modulemap $(HEADERS_DIR)/module.modulemap
	xcodebuild -create-xcframework \
		-library $(IOS_LIB) -headers $(HEADERS_DIR) \
		-library $(SIM_LIB) -headers $(HEADERS_DIR) \
		-output $(FRAMEWORK)

xcode: xcframework
	command -v xcodegen >/dev/null || brew install xcodegen
	cd SubxtDemo && xcodegen generate

all: xcode

clean:
	cargo clean
	rm -rf $(GEN_DIR) SubxtDemo/SubxtDemo.xcodeproj
