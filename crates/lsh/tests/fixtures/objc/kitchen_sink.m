// kitchen sink for objective-c highlighting
#import <Foundation/Foundation.h>
#define MAX_COUNT 10

@interface Counter : NSObject
@property (nonatomic, assign) NSInteger value;
- (instancetype)initWithValue:(NSInteger)v;
- (void)increment;
@end

@implementation Counter

- (instancetype)initWithValue:(NSInteger)v {
    self = [super init];
    if (self) {
        _value = v;
    }
    return self;
}

- (void)increment {
    for (NSInteger i = 0; i < MAX_COUNT; i++) {
        self.value += 1;
    }
    BOOL done = YES;
    id obj = nil;
    NSString *msg = @"counted";
    double ratio = 3.14e-2;
    NSLog(@"value=%ld done=%d %@", (long)self.value, done, msg);
}

@end

// A backslash-newline continues a string literal.
NSString *continued = @"one \
two";
