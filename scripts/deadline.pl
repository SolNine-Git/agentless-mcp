# perl scripts/deadline.pl SECONDS COMMAND...: run COMMAND in a new session. At the deadline (exit 124)
# or on Ctrl-C, kill each process below it and their process groups. When it ends, kill what is left of
# them. A process that leaves the session and its parent's tree (a setsid daemon) can still hold the
# output open.
use strict;
use warnings;
use POSIX ();

my $seconds = shift @ARGV;
pipe(my $reader, my $writer) or die "deadline: cannot open a pipe: $!\n";
my $leader = fork;
defined $leader or die "deadline: cannot fork: $!\n";
if ($leader == 0) {
    # The leader outlives the command, so its group id stays ours until the parent kills it.
    close $reader;
    POSIX::setsid() > 0 or die "deadline: cannot start a session: $!\n";
    my $command = fork;
    defined $command or die "deadline: cannot fork: $!\n";
    if ($command == 0) {
        close $writer;
        exec { $ARGV[0] } @ARGV;
        exit 127;
    }
    waitpid($command, 0);
    print {$writer} "$?\n";
    close $writer;
    sleep 1 while 1;
}
close $writer;

# Kill each process below the leader and each process group they are in: a child can start its own group.
sub stop {
    open my $ps, '-|', 'ps', '-A', '-o', 'pid=', '-o', 'ppid=', '-o', 'pgid='
        or die "deadline: cannot run ps: $!\n";
    my (%children, %group);
    while (my $line = <$ps>) {
        my ($pid, $ppid, $pgid) = split ' ', $line;
        push @{ $children{$ppid} }, $pid;
        $group{$pid} = $pgid;
    }
    close $ps or die "deadline: ps exited with status $?\n";
    my @tree = ($leader);
    for (my $next = 0; $next < @tree; $next++) {
        push @tree, @{ $children{ $tree[$next] } || [] };
    }
    my %groups = map { $_ => 1 } $leader, grep { defined } @group{@tree};
    kill 'KILL', @tree;
    kill '-KILL', keys %groups;
}

my $caught = '';
$SIG{$_} = sub { $caught ||= $_[0]; stop() } for qw(ALRM INT TERM HUP);
alarm $seconds;
my $status = <$reader>;
alarm 0;
stop();
waitpid($leader, 0);
if ($caught eq 'ALRM') {
    print STDERR "deadline: @ARGV ran past its $seconds s deadline: it and the processes it started were killed\n";
    exit 124;
}
if ($caught) {
    $SIG{$caught} = 'DEFAULT';
    kill $caught, $$;
}
defined $status or die "deadline: the leader of @ARGV ended before the command did\n";
exit($status & 127 ? 128 + ($status & 127) : $status >> 8);
