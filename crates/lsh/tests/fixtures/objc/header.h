// objc header -- routed to objc (not C) by content detection
#import <Foundation/Foundation.h>

@protocol Greeter <NSObject>
- (NSString *)greeting;
@end

@interface Person : NSObject <Greeter>
@property (nonatomic, copy) NSString *name;
@property (nonatomic, assign) NSInteger age;
- (instancetype)initWithName:(NSString *)name age:(NSInteger)age;
@end
